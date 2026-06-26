#!/usr/bin/env bash
# Prove + verify a Noir circuit with Groth16 (BN254) via the ported Noir-Groth16
# backend (tools/noir-groth16). Regenerates witnesses, then runs the upstream
# pipeline: nargo compile -> noir-cli interop (.r1cs/.wtns) -> snarkjs groth16
# setup/prove/verify. This is the Phase 1 proving path (Groth16, not UltraHonk).
#
# Usage: scripts/groth16-prove.sh [circuit-dir]   (default: circuits/deposit)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TOOL_DIR="$ROOT/tools/noir-groth16"
BUILD="$TOOL_DIR/build"
CIRCUIT="${1:-$ROOT/circuits/deposit}"
case "$CIRCUIT" in /*) ;; *) CIRCUIT="$PWD/$CIRCUIT" ;; esac  # resolve to absolute

[ -x "$BUILD/target/release/noir-cli" ] || "$TOOL_DIR/setup.sh"

echo "==> regenerate witnesses (inputs.json)"
cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits"

echo "==> Groth16 prove + verify: $CIRCUIT"
# run inside the tool repo so its `cargo build -p noir-cli` resolves there
( cd "$BUILD" && CIRCUIT_DIR="$CIRCUIT" PTAU_POWER="${PTAU_POWER:-14}" MIN_NARGO_VERSION=0.1.0 \
    bash scripts/run_circuit.sh )
