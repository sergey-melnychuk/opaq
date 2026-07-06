#!/usr/bin/env bash
# Phase 4 P4.2 (OPAQ.md B.12.4/B.12.8): regenerate the xburn Groth16 Solidity
# verifier + a real-proof fixture, then run OpaqPool's Foundry tests. Mirrors
# scripts/m15-evm-mint.sh's role for burn/OpaqMint — same reason: the EVM
# ecMul precompile rejects the insecure local ceremony's degenerate VK points
# (B.6), so this needs the real PPoT, and evm/src/Groth16VerifierXburn.sol +
# evm/test/XburnProof.sol are gitignored (regenerated here, not committed).
#
# NOTE: insecure test ceremony phase-2 contribution (see OPAQ.md B.6) — the
# phase-1 (PPoT) is real; exercises the OpaqPool logic.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EVM="$ROOT/evm"
WORK="$(mktemp -d)"

echo "==> seed xburn witness (dest_chain = SOLANA_CHAIN_ID, matches m19's fixture)"
OPAQ_XBURN_DEST_CHAIN=101 cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null

echo "==> fixed xburn zkey (real PPoT phase-1, non-degenerate — required for ecMul)"
PTAU="$ROOT/ceremony/.cache/powersOfTau28_hez_final_16.ptau"
bash "$ROOT/scripts/ceremony-fetch-ptau.sh" "$PTAU"
OPAQ_PTAU="$PTAU" bash "$ROOT/scripts/groth16-setup.sh" xburn "$WORK/setup_xburn" 16

echo "==> export Groth16 Solidity verifier from the xburn zkey"
snarkjs zkey export solidityverifier "$WORK/setup_xburn/circuit.zkey" "$EVM/src/Groth16VerifierXburn.sol" >/dev/null
sed -i.bak 's/contract Groth16Verifier {/contract Groth16VerifierXburn {/' "$EVM/src/Groth16VerifierXburn.sol"
rm -f "$EVM/src/Groth16VerifierXburn.sol.bak"

echo "==> generate the Solidity proof fixture from a real xburn proof"
node "$EVM/gen_fixture.mjs" "$WORK/setup_xburn/public.json" "$WORK/setup_xburn/proof.json" "$EVM/test/XburnProof.sol" XburnProof

echo "==> forge test (Poseidon parity + OpaqMint + OpaqPool)"
( cd "$EVM" && forge test -vv )
