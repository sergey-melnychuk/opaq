#!/usr/bin/env bash
# M12 / Phase 2 (OPAQ.md B.4.3): on-chain transfer (2-in/2-out join-split), e2e.
#
# Deposits note A (the gen_witness fixture), then proves + submits a transfer that
# spends A (+ a dummy) into 2 outputs, asserting the tag-3 instruction records both
# nullifiers + inserts both output commitments with NO token movement, and that a
# replay is rejected (nullifier reuse).
#
# NOTE: insecure test ceremony (see OPAQ.md B.6) — exercises the join-split logic.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/opaq"
VKMF="$ROOT/crates/groth16-verify/Cargo.toml"
WORK="$(mktemp -d)"

cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; }
cd "$ROOT"

echo "==> mint keypair + fixture params"
solana-keygen new --no-bip39-passphrase --silent --force -o "$WORK/mint.json" >/dev/null
MINT_B58="$(solana-keygen pubkey "$WORK/mint.json")"
MINT_HEX="$(node -e "const {PublicKey}=require('$ROOT/tests/node_modules/@solana/web3.js');process.stdout.write(Buffer.from(new PublicKey(process.argv[1]).toBytes()).toString('hex'))" "$MINT_B58")"
export OPAQ_MINT_HEX="$MINT_HEX"
export OPAQ_AMOUNT=1000

# gen_witness writes deposit + transfer fixtures for the SAME note A, so the
# transfer's merkle_root matches the on-chain root after depositing A at leaf 0.
echo "==> seed witnesses + fixed deposit/transfer zkeys + VKs"
cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
"$ROOT/scripts/groth16-setup.sh" deposit "$WORK/setup_deposit" 14
cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_deposit" "$WORK" >/dev/null
mv -f "$WORK/vk.rs" "$PROG/src/vk_deposit.rs"
"$ROOT/scripts/groth16-setup.sh" transfer "$WORK/setup_transfer" 17
cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_transfer" "$WORK" >/dev/null
mv -f "$WORK/vk.rs" "$PROG/src/vk_transfer.rs"

echo "==> prove deposit (note A) + transfer"
"$ROOT/scripts/groth16-prove-note.sh" deposit "$WORK/setup_deposit/circuit.zkey" "$ROOT/circuits/deposit/inputs.json" "$WORK/prove_dep"
cargo run -q --manifest-path "$VKMF" --bin emit_opaq_instruction -- \
  deposit "$WORK/prove_dep" "$ROOT/circuits/e2e_values.json" "$WORK/deposit_a.bin" >/dev/null
"$ROOT/scripts/groth16-prove-note.sh" transfer "$WORK/setup_transfer/circuit.zkey" "$ROOT/circuits/transfer/inputs.json" "$WORK/prove_xfer"
cargo run -q --manifest-path "$VKMF" --bin emit_opaq_instruction -- \
  transfer "$WORK/prove_xfer" "/dev/null" "$WORK/transfer.bin" >/dev/null

echo "==> build opaq program (fixed VKs)"
( cd "$PROG" && cargo build-sbf --tools-version v1.54 )
SBF_DEPLOY="${CARGO_TARGET_DIR:-$PROG/target}/deploy"
PROG_KP="$SBF_DEPLOY/opaq-keypair.json"

VPID=""; cleanup() { [ -n "$VPID" ] && kill "$VPID" 2>/dev/null || true; }; trap cleanup EXIT
echo "==> start validator"
solana-test-validator --reset --quiet --ledger "$WORK/ledger" >/tmp/opaq-m12-validator.log 2>&1 &
VPID=$!
solana config set --url http://127.0.0.1:8899 >/dev/null
printf "==> waiting for RPC"
for _ in $(seq 1 60); do solana cluster-version >/dev/null 2>&1 && break; printf "."; sleep 1; done
echo " ready"
[ -f ~/.config/solana/id.json ] || solana-keygen new --no-bip39-passphrase --silent -o ~/.config/solana/id.json >/dev/null
solana airdrop 5 >/dev/null 2>&1 || true

echo "==> deploy opaq"
solana program deploy "$SBF_DEPLOY/opaq.so" --program-id "$PROG_KP"

echo "==> run M12 transfer join-split e2e"
node "$ROOT/tests/m12_transfer.mjs" "$PROG_KP" "$WORK/mint.json" "$WORK/deposit_a.bin" "$WORK/transfer.bin"
