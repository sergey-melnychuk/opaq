#!/usr/bin/env bash
# P5.2 fixture (OPAQ.md B.14.6): stand up a local Solana validator with ONE
# real deposit + ONE real xburn (tag 8, Solana as SOURCE) transaction on
# chain, so the ICP attestor canister's Solana leg can be exercised against
# a genuine finalized transaction — not a canned/mocked response.
#
# Leaves the validator RUNNING in the background (does not tear down on
# exit) so a separate canister-calling step can query it live; writes the
# fixture facts (program id, tx signature, expected nullifier/dest_chain/
# out_commitment, rpc url) to $OUT_JSON for that step to consume.
#
# WARNING: like scripts/m19-mint-from-xburn.sh etc, this OVERWRITES
# programs/opaq/src/vk_deposit.rs and vk_xburn.rs with trivial local-setup
# VKs (this fixture only needs Solana-side proof verification, no real
# ceremony). Restore the real ceremony VKs afterward:
#   git checkout -- programs/opaq/src/vk_deposit.rs programs/opaq/src/vk_xburn.rs
# and verify against ceremony/transcript.md's recorded hashes before
# committing/deploying anything for real.
#
# Usage: p5.2-solana-fixture.sh [out.json]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/opaq"
VKMF="$ROOT/crates/groth16-verify/Cargo.toml"
WORK="$(mktemp -d)"
SOL_RPC="http://127.0.0.1:8899"
OUT_JSON="${1:-$WORK/p5.2-fixture.json}"
DEST_CHAIN=999 # arbitrary test chain id — this fixture never touches EVM

cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; }
cd "$ROOT"

echo "==> mint keypair + fixture params"
solana-keygen new --no-bip39-passphrase --silent --force -o "$WORK/mint.json" >/dev/null
MINT_B58="$(solana-keygen pubkey "$WORK/mint.json")"
MINT_HEX="$(node -e "const {PublicKey}=require('$ROOT/tests/node_modules/@solana/web3.js');process.stdout.write(Buffer.from(new PublicKey(process.argv[1]).toBytes()).toString('hex'))" "$MINT_B58")"
export OPAQ_MINT_HEX="$MINT_HEX"
export OPAQ_AMOUNT=1000
export OPAQ_XBURN_DEST_CHAIN="$DEST_CHAIN"

echo "==> seed witnesses (deposit + xburn, dest_chain=$DEST_CHAIN)"
cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null

echo "==> trivial local ceremony (Solana-only verification, matches M19's approach)"
"$ROOT/scripts/groth16-setup.sh" deposit "$WORK/setup_deposit" 14
cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_deposit" "$WORK" >/dev/null
mv -f "$WORK/vk.rs" "$PROG/src/vk_deposit.rs"
"$ROOT/scripts/groth16-setup.sh" xburn "$WORK/setup_xburn" 16
cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_xburn" "$WORK" >/dev/null
mv -f "$WORK/vk.rs" "$PROG/src/vk_xburn.rs"

echo "==> build opaq program"
( cd "$PROG" && cargo build-sbf --tools-version v1.54 )
SBF_DEPLOY="${CARGO_TARGET_DIR:-$PROG/target}/deploy"
PROG_KP="$SBF_DEPLOY/opaq-keypair.json"

echo "==> prove + assemble instruction blobs (deposit tag 1; xburn tag 8)"
"$ROOT/scripts/groth16-prove-note.sh" deposit "$WORK/setup_deposit/circuit.zkey" "$ROOT/circuits/deposit/inputs.json" "$WORK/prove_dep"
cargo run -q --manifest-path "$VKMF" --bin emit_opaq_instruction -- \
  deposit "$WORK/prove_dep" "$ROOT/circuits/e2e_values.json" "$WORK/deposit.bin" >/dev/null
"$ROOT/scripts/groth16-prove-note.sh" xburn "$WORK/setup_xburn/circuit.zkey" "$ROOT/circuits/xburn/inputs.json" "$WORK/prove_xburn"
cargo run -q --manifest-path "$VKMF" --bin emit_opaq_instruction -- \
  xburn-source "$WORK/prove_xburn" "$ROOT/circuits/xburn_values.json" "$WORK/xburn.bin" >/dev/null

echo "==> start validator (left running — do NOT kill after this script exits)"
solana-test-validator --reset --quiet --ledger "$WORK/ledger" >/tmp/opaq-p5.2-validator.log 2>&1 &
disown
solana config set --url "$SOL_RPC" >/dev/null
printf "==> waiting for RPC"
for _ in $(seq 1 60); do solana cluster-version >/dev/null 2>&1 && break; printf "."; sleep 1; done
echo " ready"
[ -f ~/.config/solana/id.json ] || solana-keygen new --no-bip39-passphrase --silent -o ~/.config/solana/id.json >/dev/null
solana airdrop 5 >/dev/null 2>&1 || true

echo "==> deploy opaq"
solana program deploy "$SBF_DEPLOY/opaq.so" --program-id "$PROG_KP"
PROGRAM_ID="$(solana-keygen pubkey "$PROG_KP")"

echo "==> submit deposit + xburn(tag 8) on-chain, capture the real tx signature"
node "$ROOT/tests/p5_2_fixture.mjs" \
  "$PROG_KP" "$WORK/mint.json" "$WORK/deposit.bin" "$WORK/xburn.bin" \
  "$WORK/prove_xburn" "$SOL_RPC" "$OUT_JSON"

echo "==> fixture written: $OUT_JSON"
cat "$OUT_JSON"
