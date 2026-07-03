#!/usr/bin/env bash
# M19 / Phase 4 P4.1 (OPAQ.md B.12.5): mint_from_xburn on-chain, e2e.
#
# Simulates an EVM-origin xburn proof minting a note on Solana (Solana is the
# DESTINATION here — the reverse-direction leg of the symmetric bridge). Two
# separate xburn fixtures (distinct src_nullifier via OPAQ_BLINDING) let this
# cover every P4.1 accept-criteria case in one pool: happy path, double-mint
# rejection, wrong dest_chain rejection, and unattested-nullifier rejection.
#
# NOTE: insecure test ceremony (see OPAQ.md B.6) — exercises the mint_from_xburn logic.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/opaq"
VKMF="$ROOT/crates/groth16-verify/Cargo.toml"
WORK="$(mktemp -d)"

cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; }
cd "$ROOT"

echo "==> mint keypair"
solana-keygen new --no-bip39-passphrase --silent --force -o "$WORK/mint.json" >/dev/null
MINT_B58="$(solana-keygen pubkey "$WORK/mint.json")"
MINT_HEX="$(node -e "const {PublicKey}=require('$ROOT/tests/node_modules/@solana/web3.js');process.stdout.write(Buffer.from(new PublicKey(process.argv[1]).toBytes()).toString('hex'))" "$MINT_B58")"
export OPAQ_MINT_HEX="$MINT_HEX"
export OPAQ_AMOUNT=1000
export OPAQ_XBURN_DEST_CHAIN=101 # SOLANA_CHAIN_ID (programs/opaq/src/lib.rs)

echo "==> seed two distinct xburn witnesses (fixture A: attested; fixture B: never attested)"
OPAQ_BLINDING=111111111 cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
cp "$ROOT/circuits/xburn/inputs.json" "$WORK/xburn_a_inputs.json"
cp "$ROOT/circuits/xburn_values.json" "$WORK/xburn_a_values.json"
OPAQ_BLINDING=222222222 cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
cp "$ROOT/circuits/xburn/inputs.json" "$WORK/xburn_b_inputs.json"
cp "$ROOT/circuits/xburn_values.json" "$WORK/xburn_b_values.json"

echo "==> fixed xburn zkey/VK (one setup, two notes proved against it)"
"$ROOT/scripts/groth16-setup.sh" xburn "$WORK/setup_xburn" 16
cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_xburn" "$WORK" >/dev/null
mv -f "$WORK/vk.rs" "$PROG/src/vk_xburn.rs"

echo "==> prove + assemble both mint_from_xburn instruction blobs"
"$ROOT/scripts/groth16-prove-note.sh" xburn "$WORK/setup_xburn/circuit.zkey" "$WORK/xburn_a_inputs.json" "$WORK/prove_a"
cargo run -q --manifest-path "$VKMF" --bin emit_opaq_instruction -- \
  xburn "$WORK/prove_a" "$WORK/xburn_a_values.json" "$WORK/xburn_a.bin" >/dev/null
"$ROOT/scripts/groth16-prove-note.sh" xburn "$WORK/setup_xburn/circuit.zkey" "$WORK/xburn_b_inputs.json" "$WORK/prove_b"
cargo run -q --manifest-path "$VKMF" --bin emit_opaq_instruction -- \
  xburn "$WORK/prove_b" "$WORK/xburn_b_values.json" "$WORK/xburn_b.bin" >/dev/null

echo "==> build opaq program (fixed VKs)"
( cd "$PROG" && cargo build-sbf --tools-version v1.54 )
SBF_DEPLOY="${CARGO_TARGET_DIR:-$PROG/target}/deploy"
PROG_KP="$SBF_DEPLOY/opaq-keypair.json"

VPID=""; cleanup() { [ -n "$VPID" ] && kill "$VPID" 2>/dev/null || true; }; trap cleanup EXIT
echo "==> start validator"
solana-test-validator --reset --quiet --ledger "$WORK/ledger" >/tmp/opaq-m19-validator.log 2>&1 &
VPID=$!
solana config set --url http://127.0.0.1:8899 >/dev/null
printf "==> waiting for RPC"
for _ in $(seq 1 60); do solana cluster-version >/dev/null 2>&1 && break; printf "."; sleep 1; done
echo " ready"
[ -f ~/.config/solana/id.json ] || solana-keygen new --no-bip39-passphrase --silent -o ~/.config/solana/id.json >/dev/null
solana airdrop 5 >/dev/null 2>&1 || true

echo "==> deploy opaq"
solana program deploy "$SBF_DEPLOY/opaq.so" --program-id "$PROG_KP"

echo "==> run M19 mint_from_xburn e2e"
node "$ROOT/tests/m19_mint_from_xburn.mjs" "$PROG_KP" "$WORK/xburn_a_values.json" "$WORK/xburn_a.bin" "$WORK/xburn_b.bin"
