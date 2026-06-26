#!/usr/bin/env bash
# M11 — Deploy + demo on Solana devnet (OPAQ.md B.9).
#
# Builds opaq with fixed Groth16 VKs (insecure test ceremony — B.6), deploys to
# devnet, and runs Test 1 (deposit -> withdraw round-trip) against a public RPC.
#
# Env:
#   OPAQ_DEVNET_RPC     — RPC URL (default: https://api.devnet.solana.com)
#   OPAQ_PAYER_KEYPAIR  — payer/deploy wallet (default: ~/.config/solana/id.json)
#   OPAQ_SKIP_DEPLOY=1  — skip program deploy (re-demo against an existing program)
#
# NOTE: insecure test ceremony (see OPAQ.md B.6) — devnet demo only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/opaq"
VKMF="$ROOT/crates/groth16-verify/Cargo.toml"
WORK="$(mktemp -d)"
RPC="${OPAQ_DEVNET_RPC:-https://api.devnet.solana.com}"
PAYER="${OPAQ_PAYER_KEYPAIR:-$HOME/.config/solana/id.json}"

cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; }
cd "$ROOT"

restore_vks() {
  git checkout -- "$PROG/src/vk_deposit.rs" "$PROG/src/vk_withdraw.rs" 2>/dev/null || true
}
trap 'restore_vks' EXIT

echo "==> devnet config"
echo "    rpc=$RPC"
echo "    payer=$(solana-keygen pubkey "$PAYER")"

echo "==> mint + recipient keypairs (demo SPL token)"
solana-keygen new --no-bip39-passphrase --silent --force -o "$WORK/mint.json" >/dev/null
solana-keygen new --no-bip39-passphrase --silent --force -o "$WORK/recipient.json" >/dev/null
hexpub() { node -e "const {Keypair}=require('$ROOT/tests/node_modules/@solana/web3.js');const fs=require('fs');process.stdout.write(Buffer.from(Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(process.argv[1])))).publicKey.toBytes()).toString('hex'))" "$1"; }
export OPAQ_MINT_HEX="$(hexpub "$WORK/mint.json")"
export OPAQ_RECIPIENT_HEX="$(hexpub "$WORK/recipient.json")"
export OPAQ_AMOUNT=1000
echo "    demo_mint=$OPAQ_MINT_HEX"

echo "==> gen note witnesses + fixed zkeys (deposit, withdraw)"
cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
"$ROOT/scripts/groth16-setup.sh" deposit "$WORK/setup_deposit" 14
"$ROOT/scripts/groth16-setup.sh" withdraw "$WORK/setup_withdraw" 16

emit_vk() {
  local c="$1"
  cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_$c" "$WORK" >/dev/null
  mv -f "$WORK/vk.rs" "$PROG/src/vk_$c.rs"
}
emit_vk deposit; emit_vk withdraw

prove_note() {
  local c="$1" inputs="$2" out="$3"
  local dir="$WORK/prove_$(basename "$out")"
  "$ROOT/scripts/groth16-prove-note.sh" "$c" "$WORK/setup_$c/circuit.zkey" "$inputs" "$dir"
  cargo run -q --manifest-path "$VKMF" --bin emit_opaq_instruction -- \
    "$c" "$dir" "$ROOT/circuits/e2e_values.json" "$out"
}
echo "==> prove demo note (deposit + withdraw)"
prove_note deposit  "$ROOT/circuits/deposit/inputs.json"  "$WORK/deposit.bin"
prove_note withdraw "$ROOT/circuits/withdraw/inputs.json" "$WORK/withdraw.bin"

echo "==> build opaq (fixed VKs)"
( cd "$PROG" && cargo build-sbf --tools-version v1.54 )
SBF_DEPLOY="${CARGO_TARGET_DIR:-$PROG/target}/deploy"
PROG_KP="$SBF_DEPLOY/opaq-keypair.json"
PROG_ID="$(solana-keygen pubkey "$PROG_KP")"

ensure_sol() {
  local bal num
  bal="$(solana balance -k "$PAYER" --url "$RPC" 2>/dev/null || echo "0 SOL")"
  echo "    balance=$bal"
  num="$(echo "$bal" | awk '{print $1}' | cut -d. -f1)"
  if [[ "${num:-0}" -ge 2 ]]; then
    return
  fi
  echo "==> request devnet airdrop for payer"
  solana airdrop 2 -k "$PAYER" --url "$RPC" >/dev/null 2>&1 || true
  sleep 4
  echo "    balance=$(solana balance -k "$PAYER" --url "$RPC" 2>/dev/null || echo unknown)"
}

ensure_sol

if [[ "${OPAQ_SKIP_DEPLOY:-}" != "1" ]]; then
  echo "==> deploy opaq to devnet ($PROG_ID)"
  solana program deploy "$SBF_DEPLOY/opaq.so" \
    --program-id "$PROG_KP" \
    -k "$PAYER" --url "$RPC"
else
  echo "==> skip deploy (OPAQ_SKIP_DEPLOY=1); using program $PROG_ID"
fi

echo "==> run devnet demo (Test 1 round-trip)"
node "$ROOT/tests/m11_devnet_demo.mjs" \
  "$PROG_KP" "$PAYER" "$WORK/mint.json" "$WORK/recipient.json" \
  "$WORK/deposit.bin" "$WORK/withdraw.bin" "$RPC"

mkdir -p "$ROOT/deploy"
cat >"$ROOT/deploy/devnet-latest.json" <<EOF
{
  "rpc": "$RPC",
  "program_id": "$PROG_ID",
  "deployed_at": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "demo_mint_hex": "$OPAQ_MINT_HEX",
  "note": "Insecure test-ceremony VKs (OPAQ.md B.6). Re-run scripts/m11-devnet.sh to redeploy with fresh proofs."
}
EOF
echo "==> wrote deploy/devnet-latest.json (program_id=$PROG_ID)"
