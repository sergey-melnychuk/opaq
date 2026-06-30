#!/usr/bin/env bash
# M15 / Phase 3 (OPAQ.md A.6, EVM side): the cross-chain mint, end-to-end off a
# REAL burn proof. Exports the burn circuit's Groth16 Solidity verifier, turns a
# real burn proof into a Solidity fixture, and runs the Foundry tests that drive
# OpaqMint through the pendingMint/minted lifecycle (mint, double-mint guard,
# un-pending guard, re-add guard, onlyOperator).
#
# NOTE: insecure test ceremony (see OPAQ.md B.6) — exercises the EVM mint logic.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VKMF="$ROOT/crates/groth16-verify/Cargo.toml"
EVM="$ROOT/evm"
WORK="$(mktemp -d)"

echo "==> seed burn witness + fixed burn zkey (Groth16 over BN254)"
cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
# The EVM ecMul precompile rejects degenerate IC points, so use the REAL PPoT
# phase-1 (non-degenerate, well-contributed, and pre-prepared -> fast). Cached
# in ceremony/.cache after the first ~72 MB download.
PTAU="$ROOT/ceremony/.cache/powersOfTau28_hez_final_16.ptau"
bash "$ROOT/scripts/ceremony-fetch-ptau.sh" "$PTAU"
OPAQ_PTAU="$PTAU" bash "$ROOT/scripts/groth16-setup.sh" burn "$WORK/setup_burn" 16

echo "==> export Groth16 Solidity verifier from the burn zkey"
snarkjs zkey export solidityverifier "$WORK/setup_burn/circuit.zkey" "$EVM/src/Groth16Verifier.sol" >/dev/null
# Normalize the contract name to Groth16Verifier (older snarkjs emits `Verifier`).
grep -q "contract Groth16Verifier" "$EVM/src/Groth16Verifier.sol" \
  || sed -i.bak 's/contract Verifier/contract Groth16Verifier/' "$EVM/src/Groth16Verifier.sol"
rm -f "$EVM/src/Groth16Verifier.sol.bak"

echo "==> generate the Solidity proof fixture from a real burn proof"
( cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; } )
node "$EVM/gen_fixture.mjs" "$WORK/setup_burn/public.json" "$WORK/setup_burn/proof.json" "$EVM/test/BurnProof.sol"

echo "==> forge test (OpaqMint cross-chain mint)"
( cd "$EVM" && forge test -vv )
