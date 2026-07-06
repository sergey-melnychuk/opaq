#!/usr/bin/env bash
# M20 / Phase 4 P4.3 (OPAQ.md B.12.8): the SYMMETRIC cross-chain round trip,
# live on a validator + anvil simultaneously, driven by ONE person — mirrors
# M18's structure, but the destination both ways is a real re-shielded note
# (OpaqPool.sol's tree / Solana's own tree), not OpaqMint's balance ledger.
#
# Both xburn proofs share ONE fixed zkey/VK (xburn.nr) proven over the REAL
# PPoT, so the same proof verifies on BOTH the Solana groth16-solana verifier
# AND the EVM ecMul precompile (B.6/M15's finding). Deposit uses the trivial
# ceremony (Solana-only, like M18). NOTE: insecure test ceremony phase-2
# contribution — see OPAQ.md B.6.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/opaq"
VKMF="$ROOT/crates/groth16-verify/Cargo.toml"
EVM="$ROOT/evm"
WORK="$(mktemp -d)"
SOL_RPC="http://127.0.0.1:8899"
EVM_PORT=8547; EVM_RPC="http://127.0.0.1:$EVM_PORT"
CHAIN_ID=31337 # anvil's own default chain id — this pool's mintFromXburn dest_chain

cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; }
cd "$ROOT"

echo "==> mint keypair + fixture params (must match the real deposit args, B.5.2's binding)"
solana-keygen new --no-bip39-passphrase --silent --force -o "$WORK/mint.json" >/dev/null
MINT_B58="$(solana-keygen pubkey "$WORK/mint.json")"
MINT_HEX="$(node -e "const {PublicKey}=require('$ROOT/tests/node_modules/@solana/web3.js');process.stdout.write(Buffer.from(new PublicKey(process.argv[1]).toBytes()).toString('hex'))" "$MINT_B58")"
export OPAQ_MINT_HEX="$MINT_HEX"
export OPAQ_AMOUNT=1000

echo "==> seed witnesses (both xburn legs, dest_chain = anvil's chain id $CHAIN_ID)"
OPAQ_XBURN_DEST_CHAIN="$CHAIN_ID" cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
cp "$ROOT/circuits/xburn/inputs.json" "$WORK/xburn1_inputs.json"
cp "$ROOT/circuits/xburn_values.json" "$WORK/xburn1_values.json"
cp "$ROOT/circuits/xburn2_inputs.json" "$WORK/xburn2_inputs.json"
cp "$ROOT/circuits/xburn2_values.json" "$WORK/xburn2_values.json"

echo "==> deposit zkey (trivial ceremony — Solana-only verification)"
"$ROOT/scripts/groth16-setup.sh" deposit "$WORK/setup_deposit" 14
cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_deposit" "$WORK" >/dev/null
mv -f "$WORK/vk.rs" "$PROG/src/vk_deposit.rs"

echo "==> xburn zkey (REAL PPoT — needed for the EVM ecMul precompile too)"
PTAU="$ROOT/ceremony/.cache/powersOfTau28_hez_final_16.ptau"
bash "$ROOT/scripts/ceremony-fetch-ptau.sh" "$PTAU"
OPAQ_PTAU="$PTAU" bash "$ROOT/scripts/groth16-setup.sh" xburn "$WORK/setup_xburn" 16
cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_xburn" "$WORK" >/dev/null
mv -f "$WORK/vk.rs" "$PROG/src/vk_xburn.rs"

echo "==> build opaq program (real vk_xburn)"
( cd "$PROG" && cargo build-sbf --tools-version v1.54 )
SBF_DEPLOY="${CARGO_TARGET_DIR:-$PROG/target}/deploy"
PROG_KP="$SBF_DEPLOY/opaq-keypair.json"

echo "==> export Groth16 Solidity verifier (same xburn zkey) + forge build"
snarkjs zkey export solidityverifier "$WORK/setup_xburn/circuit.zkey" "$EVM/src/Groth16VerifierXburn.sol" >/dev/null
sed -i.bak 's/contract Groth16Verifier {/contract Groth16VerifierXburn {/' "$EVM/src/Groth16VerifierXburn.sol"
rm -f "$EVM/src/Groth16VerifierXburn.sol.bak"
( cd "$EVM" && forge build -q )

echo "==> prove both xburn witnesses against the ONE shared zkey"
"$ROOT/scripts/groth16-prove-note.sh" xburn "$WORK/setup_xburn/circuit.zkey" "$WORK/xburn1_inputs.json" "$WORK/prove1"
"$ROOT/scripts/groth16-prove-note.sh" xburn "$WORK/setup_xburn/circuit.zkey" "$WORK/xburn2_inputs.json" "$WORK/prove2"

echo "==> assemble Solana instruction blobs (deposit; xburn tag 8; mint_from_xburn tag 7)"
"$ROOT/scripts/groth16-prove-note.sh" deposit "$WORK/setup_deposit/circuit.zkey" "$ROOT/circuits/deposit/inputs.json" "$WORK/prove_dep"
cargo run -q --manifest-path "$VKMF" --bin emit_opaq_instruction -- \
  deposit "$WORK/prove_dep" "$ROOT/circuits/e2e_values.json" "$WORK/deposit.bin" >/dev/null
cargo run -q --manifest-path "$VKMF" --bin emit_opaq_instruction -- \
  xburn-source "$WORK/prove1" "$WORK/xburn1_values.json" "$WORK/xburn1_solana.bin" >/dev/null
cargo run -q --manifest-path "$VKMF" --bin emit_opaq_instruction -- \
  xburn "$WORK/prove2" "$WORK/xburn2_values.json" "$WORK/xburn2_solana.bin" >/dev/null

VPID=""; APID=""
cleanup() { [ -n "$VPID" ] && kill "$VPID" 2>/dev/null; [ -n "$APID" ] && kill "$APID" 2>/dev/null; true; }
trap cleanup EXIT

echo "==> start validator (Solana) + anvil (EVM, chain-id $CHAIN_ID, port $EVM_PORT)"
solana-test-validator --reset --quiet --ledger "$WORK/ledger" >/tmp/opaq-m20-validator.log 2>&1 &
VPID=$!
anvil --chain-id "$CHAIN_ID" --port "$EVM_PORT" --silent >/tmp/opaq-m20-anvil.log 2>&1 &
APID=$!
solana config set --url "$SOL_RPC" >/dev/null
printf "==> waiting for RPCs"
for _ in $(seq 1 60); do solana cluster-version >/dev/null 2>&1 && break; printf "."; sleep 1; done
for _ in $(seq 1 40); do cast block-number --rpc-url "$EVM_RPC" >/dev/null 2>&1 && break; sleep 0.5; done
echo " ready"
[ -f ~/.config/solana/id.json ] || solana-keygen new --no-bip39-passphrase --silent -o ~/.config/solana/id.json >/dev/null
solana airdrop 5 >/dev/null 2>&1 || true

echo "==> deploy opaq (Solana)"
solana program deploy "$SBF_DEPLOY/opaq.so" --program-id "$PROG_KP"

echo "==> deploy Groth16VerifierXburn + OpaqPool (EVM)"
OP_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
OP_ADDR=$(cast wallet address --private-key "$OP_KEY")
forge_deploy() {
  local spec="$1"; shift; local out
  out=$(cd "$EVM" && forge create "$spec" --rpc-url "$EVM_RPC" --private-key "$OP_KEY" --broadcast --json "$@" 2>/tmp/opaq-m20-forge.err) || true
  [ -n "$out" ] || { echo "forge create $spec failed:"; cat /tmp/opaq-m20-forge.err; exit 1; }
  printf '%s' "$out" | node -e 'let d="";process.stdin.on("data",c=>d+=c).on("end",()=>{const j=d.slice(d.indexOf("{"),d.lastIndexOf("}")+1);process.stdout.write(JSON.parse(j).deployedTo)})'
}
VERIFIER=$(forge_deploy "src/Groth16VerifierXburn.sol:Groth16VerifierXburn")
# PoseidonT3's hash() is `public` (poseidon-solidity's own design, so ONE deployed
# copy can be linked by many consumers) — forge test auto-links this, but a plain
# `forge create` doesn't, so deploy it first and link explicitly.
POSEIDON=$(forge_deploy "src/PoseidonT3.sol:PoseidonT3")
POOL=$(forge_deploy "src/OpaqPool.sol:OpaqPool" \
  --libraries "src/PoseidonT3.sol:PoseidonT3:$POSEIDON" \
  --constructor-args "$VERIFIER" "$OP_ADDR")
echo "  opaqPool=$POOL (operator=$OP_ADDR, poseidonT3=$POSEIDON)"

echo "==> run M20 symmetric round trip"
node "$ROOT/tests/m20_symmetric_roundtrip.mjs" \
  "$PROG_KP" "$WORK/mint.json" "$WORK/deposit.bin" \
  "$WORK/xburn1_solana.bin" "$WORK/xburn2_solana.bin" \
  "$WORK/prove1" "$WORK/prove2" \
  "$EVM_RPC" "$POOL" "$OP_KEY" "$SOL_RPC"
