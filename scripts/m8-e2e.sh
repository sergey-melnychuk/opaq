#!/usr/bin/env bash
# M8 end-to-end (OPAQ.md B.8): real deposit -> withdraw round-trip + double-spend
# rejection on a fresh validator. Generates real proofs for a freshly-minted SPL
# token, regenerates the embedded VKs from the SAME runs (so they match), builds
# + deploys the native opaq program, and drives the round-trip from JS.
#
# NOTE: insecure test ceremony (see OPAQ.md B.6) — exercises program logic only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/opaq"
GROTH="$ROOT/tools/noir-groth16/build/target/groth16/proof"
VKMF="$ROOT/crates/groth16-verify/Cargo.toml"
WORK="$(mktemp -d)"

cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; }
cd "$ROOT"

echo "==> mint + recipient keypairs"
solana-keygen new --no-bip39-passphrase --silent --force -o "$WORK/mint.json" >/dev/null
solana-keygen new --no-bip39-passphrase --silent --force -o "$WORK/recipient.json" >/dev/null
hexpub() { node -e "const {Keypair}=require('$ROOT/tests/node_modules/@solana/web3.js');const fs=require('fs');process.stdout.write(Buffer.from(Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(process.argv[1])))).publicKey.toBytes()).toString('hex'))" "$1"; }
export OPAQ_MINT_HEX="$(hexpub "$WORK/mint.json")"
export OPAQ_RECIPIENT_HEX="$(hexpub "$WORK/recipient.json")"
export OPAQ_AMOUNT=1000
echo "    mint=$OPAQ_MINT_HEX"

# Note A's witnesses (deposit + withdraw), then FIX a zkey/VK per circuit so
# multiple notes share one verifier (the non-determinism fix — B.9 M8).
echo "==> gen note A witnesses"
cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
echo "==> setup fixed zkeys (deposit, withdraw)"
"$ROOT/scripts/groth16-setup.sh" deposit "$WORK/setup_deposit" 14
"$ROOT/scripts/groth16-setup.sh" withdraw "$WORK/setup_withdraw" 16

emit_vk() {  # embed the VK from a fixed-zkey setup dir
  local c="$1"
  cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_$c" "$WORK" >/dev/null
  mv -f "$WORK/vk.rs" "$PROG/src/vk_$c.rs"
}
emit_vk deposit; emit_vk withdraw

prove_note() {  # circuit, inputs.json, out-blob — prove against the fixed zkey
  local c="$1" inputs="$2" out="$3" dir="$WORK/prove_$(basename "$out")"
  "$ROOT/scripts/groth16-prove-note.sh" "$c" "$WORK/setup_$c/circuit.zkey" "$inputs" "$dir"
  cargo run -q --manifest-path "$VKMF" --bin emit_opaq_instruction -- \
    "$c" "$dir" "$ROOT/circuits/e2e_values.json" "$out"
}
echo "==> prove note A (deposit + withdraw)"
prove_note deposit  "$ROOT/circuits/deposit/inputs.json"  "$WORK/deposit.bin"
prove_note withdraw "$ROOT/circuits/withdraw/inputs.json" "$WORK/withdraw.bin"

# Note B: a second, distinct deposit (different blinding) proved against the SAME
# deposit zkey — moves the root so the withdraw of A exercises stale-root (Test 4).
echo "==> gen + prove note B (deposit)"
OPAQ_BLINDING=222222222 cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
prove_note deposit "$ROOT/circuits/deposit/inputs.json" "$WORK/deposit_b.bin"

echo "==> build opaq (fixed VKs)"
( cd "$PROG" && cargo build-sbf --tools-version v1.54 )

VPID=""; cleanup() { [ -n "$VPID" ] && kill "$VPID" 2>/dev/null || true; }; trap cleanup EXIT
echo "==> start validator"
solana-test-validator --reset --quiet --ledger "$WORK/ledger" >/tmp/opaq-m8-validator.log 2>&1 &
VPID=$!
solana config set --url http://127.0.0.1:8899 >/dev/null
printf "==> waiting for RPC"
for _ in $(seq 1 60); do solana cluster-version >/dev/null 2>&1 && break; printf "."; sleep 1; done
echo " ready"
[ -f ~/.config/solana/id.json ] || solana-keygen new --no-bip39-passphrase --silent -o ~/.config/solana/id.json >/dev/null
solana airdrop 5 >/dev/null 2>&1 || true

echo "==> deploy opaq"
solana program deploy "$PROG/target/deploy/opaq.so" \
  --program-id "$PROG/target/deploy/opaq-keypair.json"

echo "==> run e2e"
node "$ROOT/tests/m8_e2e.mjs" \
  "$PROG/target/deploy/opaq-keypair.json" "$WORK/mint.json" "$WORK/recipient.json" \
  "$WORK/deposit.bin" "$WORK/deposit_b.bin" "$WORK/withdraw.bin"
