#!/usr/bin/env bash
# M18 / Phase 3 (OPAQ.md B.11 #3): the FORWARD cross-chain round-trip, end-to-end,
# driven by ONE person with ONE proof. Deposit a note on Solana, `opaq burn --submit
# --prove-dir` (records the nullifier on Solana + keeps the proof), then feed that
# SAME proof to evm/mint.mjs to mint on a live EVM chain (anvil) — no relayer.
#
# The burn zkey uses the REAL PPoT ptau so the single proof verifies on BOTH the
# Solana program (real vk_burn) AND the EVM ecMul precompile. Deposit uses the
# trivial ceremony (Solana-only). NOTE: insecure test ceremony — see OPAQ.md B.6.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/opaq"
VKMF="$ROOT/crates/groth16-verify/Cargo.toml"
EVM="$ROOT/evm"
WORK="$(mktemp -d)"
PROOFDIR="$WORK/burnproof"; mkdir -p "$PROOFDIR"
export OPAQ_PASSPHRASE="opaq-m18-test-passphrase"
SOL_RPC="http://127.0.0.1:8899"
EVM_PORT=8546; EVM_RPC="http://127.0.0.1:$EVM_PORT"
CHAIN_ID=1
DEST_ADDR="0x1111111111111111111111111111111111111111"

cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; }
cd "$ROOT"

echo "==> build opaq CLI + mint keypair"
cargo build -q -p opaq-prover
OPAQ="${CARGO_TARGET_DIR:-$ROOT/target}/debug/opaq"
solana-keygen new --no-bip39-passphrase --silent --force -o "$WORK/mint.json" >/dev/null

echo "==> seed witnesses; deposit zkey (trivial) + burn zkey (REAL ptau)"
cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
"$ROOT/scripts/groth16-setup.sh" deposit "$WORK/setup_deposit" 14
cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_deposit" "$WORK" >/dev/null
mv -f "$WORK/vk.rs" "$PROG/src/vk_deposit.rs"
PTAU="$ROOT/ceremony/.cache/powersOfTau28_hez_final_16.ptau"
bash "$ROOT/scripts/ceremony-fetch-ptau.sh" "$PTAU"
OPAQ_PTAU="$PTAU" bash "$ROOT/scripts/groth16-setup.sh" burn "$WORK/setup_burn" 16
cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_burn" "$WORK" >/dev/null
mv -f "$WORK/vk.rs" "$PROG/src/vk_burn.rs"

echo "==> build opaq program (real vk_burn)"
( cd "$PROG" && cargo build-sbf --tools-version v1.54 )
SBF_DEPLOY="${CARGO_TARGET_DIR:-$PROG/target}/deploy"
PROG_KP="$SBF_DEPLOY/opaq-keypair.json"

echo "==> export Groth16 verifier (same burn zkey) + forge build"
snarkjs zkey export solidityverifier "$WORK/setup_burn/circuit.zkey" "$EVM/src/Groth16Verifier.sol" >/dev/null
grep -q "contract Groth16Verifier" "$EVM/src/Groth16Verifier.sol" \
  || { sed -i.bak 's/contract Verifier/contract Groth16Verifier/' "$EVM/src/Groth16Verifier.sol"; rm -f "$EVM/src/Groth16Verifier.sol.bak"; }
( cd "$EVM" && forge build -q )

VPID=""; APID=""
cleanup() { [ -n "$VPID" ] && kill "$VPID" 2>/dev/null; [ -n "$APID" ] && kill "$APID" 2>/dev/null; true; }
trap cleanup EXIT

echo "==> start validator (Solana) + anvil (EVM, chain-id $CHAIN_ID, port $EVM_PORT)"
solana-test-validator --reset --quiet --ledger "$WORK/ledger" >/tmp/opaq-m18-validator.log 2>&1 &
VPID=$!
anvil --chain-id "$CHAIN_ID" --port "$EVM_PORT" --silent >/tmp/opaq-m18-anvil.log 2>&1 &
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

echo "==> deploy Groth16Verifier + OpaqMint (EVM)"
OP_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
USER_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
OP_ADDR=$(cast wallet address --private-key "$OP_KEY")
forge_deploy() {
  local spec="$1"; shift; local out
  out=$(cd "$EVM" && forge create "$spec" --rpc-url "$EVM_RPC" --private-key "$OP_KEY" --broadcast --json "$@" 2>/tmp/opaq-m18-forge.err) || true
  [ -n "$out" ] || { echo "forge create $spec failed:"; cat /tmp/opaq-m18-forge.err; exit 1; }
  printf '%s' "$out" | node -e 'let d="";process.stdin.on("data",c=>d+=c).on("end",()=>{const j=d.slice(d.indexOf("{"),d.lastIndexOf("}")+1);process.stdout.write(JSON.parse(j).deployedTo)})'
}
VERIFIER=$(forge_deploy "src/Groth16Verifier.sol:Groth16Verifier")
MINT=$(forge_deploy "src/OpaqMint.sol:OpaqMint" --constructor-args "$VERIFIER" "$OP_ADDR")
echo "  opaqMint=$MINT (operator=$OP_ADDR)"

echo "==> Solana side: deposit -> burn --prove-dir (records nullifier, keeps proof)"
OPAQ_DEPOSIT_ZKEY="$WORK/setup_deposit/circuit.zkey" \
OPAQ_BURN_ZKEY="$WORK/setup_burn/circuit.zkey" \
OPAQ_ROOT="$ROOT" \
node "$ROOT/tests/m18_roundtrip.mjs" "$PROG_KP" "$WORK/mint.json" "$OPAQ" "$PROOFDIR" "$CHAIN_ID" "$DEST_ADDR"

# From the burn's public inputs [merkle_root, nullifier, token_id, amount, dest_chain, dest_address]:
read -r NULLIFIER TOKEN_ID AMOUNT DEST_CHAIN TO < <(node -e '
  const fs=require("fs");
  const s=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));
  const h=x=>"0x"+BigInt(x).toString(16).padStart(64,"0");
  const to="0x"+(BigInt(s[5])&((1n<<160n)-1n)).toString(16).padStart(40,"0");
  console.log([h(s[1]),h(s[2]),s[3],s[4],to].join(" "));
' "$PROOFDIR/public.json")

echo "==> EVM side: operator addPending (binding the full attested tuple — B.12.5) -> owner mints the SAME proof (self-served)"
cast send "$MINT" "addPending(bytes32,bytes32,uint256,uint256,address)" \
  "$NULLIFIER" "$TOKEN_ID" "$AMOUNT" "$DEST_CHAIN" "$TO" --rpc-url "$EVM_RPC" --private-key "$OP_KEY" >/dev/null
TXH=$(node "$EVM/mint.mjs" "$EVM_RPC" "$MINT" "$USER_KEY" "$PROOFDIR/public.json" "$PROOFDIR/proof.json")
echo "  mint tx $TXH"

echo "==> assert EVM balance credited from the Solana burn proof"
BAL=$(cast call "$MINT" "balanceOf(bytes32,address)(uint256)" "$TOKEN_ID" "$TO" --rpc-url "$EVM_RPC")
[ "$BAL" = "$AMOUNT" ] || { echo "FAIL: EVM balance $BAL != burned amount $AMOUNT"; exit 1; }
echo "  OK  minted $AMOUNT to $TO on EVM"

echo
echo "M18 PASSED — forward cross-chain round-trip, one person, one proof:"
echo "  Solana deposit -> burn (nullifier recorded, value locked) -> EVM mint (same proof), no relayer."
