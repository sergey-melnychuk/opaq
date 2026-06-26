#!/usr/bin/env bash
# Test 5 — Root ring buffer overflow (OPAQ.md B.8) on a fresh validator. Proves
# note A (deposit + withdraw) against a fixed zkey/VK, then drives 33 deposits +
# an evicted-root withdraw from JS, asserting a clear E_UNKNOWN_ROOT rejection.
#
# NOTE: insecure test ceremony (see OPAQ.md B.6) — exercises program logic only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/opaq"
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

echo "==> gen note A witnesses + fixed zkeys (deposit, withdraw)"
cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
"$ROOT/scripts/groth16-setup.sh" deposit "$WORK/setup_deposit" 14
"$ROOT/scripts/groth16-setup.sh" withdraw "$WORK/setup_withdraw" 16

emit_vk() {  # embed the VK from a fixed-zkey setup dir
  local c="$1"
  cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_$c" "$WORK" >/dev/null
  mv -f "$WORK/vk.rs" "$PROG/src/vk_$c.rs"
}
emit_vk deposit; emit_vk withdraw

prove_note() {  # circuit, inputs.json, out-blob — prove against the fixed zkey
  local c="$1" inputs="$2" out="$3"
  local dir="$WORK/prove_$(basename "$out")"
  "$ROOT/scripts/groth16-prove-note.sh" "$c" "$WORK/setup_$c/circuit.zkey" "$inputs" "$dir"
  cargo run -q --manifest-path "$VKMF" --bin emit_opaq_instruction -- \
    "$c" "$dir" "$ROOT/circuits/e2e_values.json" "$out"
}
echo "==> prove note A (deposit + withdraw)"
prove_note deposit  "$ROOT/circuits/deposit/inputs.json"  "$WORK/deposit_a.bin"
prove_note withdraw "$ROOT/circuits/withdraw/inputs.json" "$WORK/withdraw_a.bin"

echo "==> build opaq (fixed VKs)"
( cd "$PROG" && cargo build-sbf --tools-version v1.54 )
# build-sbf honors CARGO_TARGET_DIR (a sandboxed/shared cache) — deploy from there.
SBF_DEPLOY="${CARGO_TARGET_DIR:-$PROG/target}/deploy"
PROG_KP="$SBF_DEPLOY/opaq-keypair.json"

VPID=""; cleanup() { [ -n "$VPID" ] && kill "$VPID" 2>/dev/null || true; }; trap cleanup EXIT
echo "==> start validator"
solana-test-validator --reset --quiet --ledger "$WORK/ledger" >/tmp/opaq-test5-validator.log 2>&1 &
VPID=$!
solana config set --url http://127.0.0.1:8899 >/dev/null
printf "==> waiting for RPC"
for _ in $(seq 1 60); do solana cluster-version >/dev/null 2>&1 && break; printf "."; sleep 1; done
echo " ready"
[ -f ~/.config/solana/id.json ] || solana-keygen new --no-bip39-passphrase --silent -o ~/.config/solana/id.json >/dev/null
solana airdrop 5 >/dev/null 2>&1 || true

echo "==> deploy opaq"
solana program deploy "$SBF_DEPLOY/opaq.so" --program-id "$PROG_KP"

echo "==> run Test 5"
node "$ROOT/tests/test5_ringbuffer.mjs" \
  "$PROG_KP" "$WORK/mint.json" "$WORK/recipient.json" \
  "$WORK/deposit_a.bin" "$WORK/withdraw_a.bin"
