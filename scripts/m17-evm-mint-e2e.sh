#!/usr/bin/env bash
# M17 / Phase 3 (OPAQ.md B.11 #3, EVM side): drive the cross-chain MINT self-served
# on a LIVE chain (anvil). The operator mirrors the burned nullifier (addPending —
# the A.9 attestation), then the note owner submits their OWN mint from a real burn
# proof via evm/mint.mjs — no relayer. Asserts the EVM balance is credited and the
# double-mint guard holds.
#
# NOTE: insecure test ceremony (B.6). Uses the REAL PPoT ptau (the EVM ecMul
# precompile rejects the trivial ceremony's degenerate IC points).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EVM="$ROOT/evm"
WORK="$(mktemp -d)"
PORT=8546   # off the default 8545 so a stray anvil can't shadow this run
RPC="http://127.0.0.1:$PORT"
CHAIN_ID=1  # gen_witness burns to dest_chain=1 (Ethereum mainnet); anvil mirrors it

cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; }
cd "$ROOT"

echo "==> seed burn witness + real burn zkey + proof"
cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
PTAU="$ROOT/ceremony/.cache/powersOfTau28_hez_final_16.ptau"
bash "$ROOT/scripts/ceremony-fetch-ptau.sh" "$PTAU"
OPAQ_PTAU="$PTAU" bash "$ROOT/scripts/groth16-setup.sh" burn "$WORK/setup_burn" 16
PUB="$WORK/setup_burn/public.json"; PROOF="$WORK/setup_burn/proof.json"

echo "==> export Groth16 verifier + forge build"
snarkjs zkey export solidityverifier "$WORK/setup_burn/circuit.zkey" "$EVM/src/Groth16Verifier.sol" >/dev/null
grep -q "contract Groth16Verifier" "$EVM/src/Groth16Verifier.sol" \
  || { sed -i.bak 's/contract Verifier/contract Groth16Verifier/' "$EVM/src/Groth16Verifier.sol"; rm -f "$EVM/src/Groth16Verifier.sol.bak"; }
( cd "$EVM" && forge build -q )

echo "==> start anvil (chain-id $CHAIN_ID, port $PORT)"
anvil --chain-id "$CHAIN_ID" --port "$PORT" --silent >/tmp/opaq-m17-anvil.log 2>&1 &
APID=$!; trap 'kill $APID 2>/dev/null || true' EXIT
for _ in $(seq 1 40); do cast block-number --rpc-url "$RPC" >/dev/null 2>&1 && break; sleep 0.5; done
kill -0 "$APID" 2>/dev/null && cast block-number --rpc-url "$RPC" >/dev/null 2>&1 \
  || { echo "anvil failed to start:"; cat /tmp/opaq-m17-anvil.log; exit 1; }

# anvil deterministic dev accounts: 0 = operator, 1 = the minting note owner.
OP_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
USER_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
OP_ADDR=$(cast wallet address --private-key "$OP_KEY")

forge_deploy() {  # $1=spec, rest passthrough (e.g. --constructor-args …); echoes deployedTo
  local spec="$1"; shift
  local out
  out=$(cd "$EVM" && forge create "$spec" --rpc-url "$RPC" --private-key "$OP_KEY" --broadcast --json "$@" 2>/tmp/opaq-m17-forge.err) || true
  [ -n "$out" ] || { echo "forge create $spec failed; stderr:" >&2; cat /tmp/opaq-m17-forge.err >&2; exit 1; }
  printf '%s' "$out" | node -e 'let d="";process.stdin.on("data",c=>d+=c).on("end",()=>{const j=d.slice(d.indexOf("{"),d.lastIndexOf("}")+1);process.stdout.write(JSON.parse(j).deployedTo)})'
}
echo "==> deploy Groth16Verifier + OpaqMint(operator=$OP_ADDR)"
VERIFIER=$(forge_deploy "src/Groth16Verifier.sol:Groth16Verifier")
MINT=$(forge_deploy "src/OpaqMint.sol:OpaqMint" --constructor-args "$VERIFIER" "$OP_ADDR")
echo "  verifier=$VERIFIER  opaqMint=$MINT"

# public.json = [merkle_root, nullifier, token_id, amount, dest_chain, dest_address] (decimal strings).
read -r NULLIFIER TOKEN_ID AMOUNT TO < <(node -e '
  const fs=require("fs");
  const s=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));
  const h=x=>"0x"+BigInt(x).toString(16).padStart(64,"0");
  const to="0x"+(BigInt(s[5])&((1n<<160n)-1n)).toString(16).padStart(40,"0");
  console.log([h(s[1]),h(s[2]),s[3],to].join(" "));
' "$PUB")
echo "  nullifier=$NULLIFIER  token=$TOKEN_ID  amount=$AMOUNT  to=$TO"

echo "==> operator mirrors the burned nullifier (addPending)"
cast send "$MINT" "addPending(bytes32)" "$NULLIFIER" --rpc-url "$RPC" --private-key "$OP_KEY" >/dev/null

echo "==> note owner mints SELF-SERVED (evm/mint.mjs, no relayer)"
TXH=$(node "$EVM/mint.mjs" "$RPC" "$MINT" "$USER_KEY" "$PUB" "$PROOF")
echo "  mint tx $TXH"

echo "==> assert EVM balance credited + guards"
BAL=$(cast call "$MINT" "balanceOf(bytes32,address)(uint256)" "$TOKEN_ID" "$TO" --rpc-url "$RPC")
MINTED=$(cast call "$MINT" "minted(bytes32)(bool)" "$NULLIFIER" --rpc-url "$RPC")
[ "$BAL" = "$AMOUNT" ] || { echo "FAIL: balance $BAL != minted amount $AMOUNT"; exit 1; }
[ "$MINTED" = "true" ]  || { echo "FAIL: minted[nullifier] not set"; exit 1; }
echo "  OK  minted $AMOUNT to $TO; minted[nullifier]=true"

echo "==> double-mint must be rejected (nullifier consumed)"
if node "$EVM/mint.mjs" "$RPC" "$MINT" "$USER_KEY" "$PUB" "$PROOF" >/dev/null 2>&1; then
  echo "FAIL: second mint should have been rejected"; exit 1
fi
echo "  OK  double-mint rejected"

echo
echo "M17 PASSED — Phase 3 cross-chain MINT driven self-served on a live chain:"
echo "  operator addPending -> owner mint (real burn proof, no relayer) -> balance credited, double-mint blocked."
