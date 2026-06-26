#!/usr/bin/env bash
# M10 / Test 7 (OPAQ.md B.8): zero-infra read path, end-to-end on a validator.
#
# Drives the REAL client flow: `opaq deposit` mints encrypted notes + circuit
# inputs, those are proved against a fixed deposit zkey and landed on-chain, and
# then a fresh RPC-only client (tests/read_path.mjs) reconstructs note A's Merkle
# path via `opaq withdraw --leaves` and asserts it hits a known on-chain root.
#
# NOTE: insecure test ceremony (see OPAQ.md B.6) — exercises the read path only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/opaq"
VKMF="$ROOT/crates/groth16-verify/Cargo.toml"
WORK="$(mktemp -d)"
export OPAQ_PASSPHRASE="opaq-m10-test-passphrase"

cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; }
cd "$ROOT"

echo "==> build opaq CLI + keypairs"
cargo build -q -p opaq-prover
# Honor CARGO_TARGET_DIR (a sandboxed/shared target cache) for the built binary.
OPAQ="${CARGO_TARGET_DIR:-$ROOT/target}/debug/opaq"
solana-keygen new --no-bip39-passphrase --silent --force -o "$WORK/mint.json" >/dev/null
solana-keygen new --no-bip39-passphrase --silent --force -o "$WORK/recipient.json" >/dev/null
MINT_B58="$(solana-keygen pubkey "$WORK/mint.json")"
MINT_HEX="$(node -e "const {PublicKey}=require('$ROOT/tests/node_modules/@solana/web3.js');process.stdout.write(Buffer.from(new PublicKey(process.argv[1]).toBytes()).toString('hex'))" "$MINT_B58")"
export OPAQ_MINT_HEX="$MINT_HEX"
export OPAQ_AMOUNT=1000
echo "    mint=$MINT_B58"

# Fixed deposit + withdraw zkeys/VKs (setup is circuit-only, so any later note
# proves against them) — the withdraw zkey lets us prove the RPC-reconstructed
# witness and submit a real withdraw, closing the zero-infra loop.
echo "==> seed witness + fixed deposit/withdraw zkeys/VKs"
cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
"$ROOT/scripts/groth16-setup.sh" deposit "$WORK/setup_deposit" 14
cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_deposit" "$WORK" >/dev/null
mv -f "$WORK/vk.rs" "$PROG/src/vk_deposit.rs"
"$ROOT/scripts/groth16-setup.sh" withdraw "$WORK/setup_withdraw" 16
cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_withdraw" "$WORK" >/dev/null
mv -f "$WORK/vk.rs" "$PROG/src/vk_withdraw.rs"

# Mint a real encrypted note via the CLI, prove it, and assemble its on-chain
# deposit instruction. Each call uses fresh secrets => a distinct commitment.
mint_note() {  # note-out, bin-out
  local note="$1" out="$2"
  local dir="$WORK/prove_$(basename "$out")"
  local inputs="$WORK/$(basename "$note").inputs.json"
  "$OPAQ" deposit --token "$MINT_B58" --amount "$OPAQ_AMOUNT" --note "$note" --inputs-out "$inputs" >/dev/null
  local commitment
  commitment="$(node -e "process.stdout.write(JSON.parse(require('fs').readFileSync(process.argv[1])).new_commitment.replace(/^0x/,''))" "$inputs")"
  # emit_opaq_instruction reads every sidecar field; deposit only uses these three.
  cat >"$dir.sidecar.json" <<EOF
{"mint_hex":"$MINT_HEX","amount":$OPAQ_AMOUNT,"commitment":"$commitment","nullifier":"$(printf '00%.0s' {1..32})","merkle_root":"$(printf '00%.0s' {1..32})","recipient_hex":"$(printf '00%.0s' {1..32})"}
EOF
  "$ROOT/scripts/groth16-prove-note.sh" deposit "$WORK/setup_deposit/circuit.zkey" "$inputs" "$dir"
  cargo run -q --manifest-path "$VKMF" --bin emit_opaq_instruction -- \
    deposit "$dir" "$dir.sidecar.json" "$out" >/dev/null
}
echo "==> mint + prove note A and note B"
mint_note "$WORK/noteA.json" "$WORK/deposit_a.bin"
mint_note "$WORK/noteB.json" "$WORK/deposit_b.bin"

echo "==> build opaq program (fixed deposit VK)"
( cd "$PROG" && cargo build-sbf --tools-version v1.54 )
# Honor CARGO_TARGET_DIR if set (e.g. a sandboxed/shared target cache) — that's
# where build-sbf actually drops deploy/opaq.so + the program keypair.
SBF_DEPLOY="${CARGO_TARGET_DIR:-$PROG/target}/deploy"
PROG_KP="$SBF_DEPLOY/opaq-keypair.json"

VPID=""; cleanup() { [ -n "$VPID" ] && kill "$VPID" 2>/dev/null || true; }; trap cleanup EXIT
echo "==> start validator"
solana-test-validator --reset --quiet --ledger "$WORK/ledger" >/tmp/opaq-m10-validator.log 2>&1 &
VPID=$!
solana config set --url http://127.0.0.1:8899 >/dev/null
printf "==> waiting for RPC"
for _ in $(seq 1 60); do solana cluster-version >/dev/null 2>&1 && break; printf "."; sleep 1; done
echo " ready"
[ -f ~/.config/solana/id.json ] || solana-keygen new --no-bip39-passphrase --silent -o ~/.config/solana/id.json >/dev/null
solana airdrop 5 >/dev/null 2>&1 || true

echo "==> deploy opaq"
solana program deploy "$SBF_DEPLOY/opaq.so" --program-id "$PROG_KP"

echo "==> run M10 zero-infra read path (+ withdraw via reconstructed path)"
OPAQ_WITHDRAW_ZKEY="$WORK/setup_withdraw/circuit.zkey" \
OPAQ_VKMF="$VKMF" OPAQ_ROOT="$ROOT" \
node "$ROOT/tests/m10_zero_infra.mjs" \
  "$PROG_KP" "$WORK/mint.json" "$WORK/recipient.json" \
  "$WORK/deposit_a.bin" "$WORK/deposit_b.bin" "$WORK/noteA.json" "$OPAQ"
