#!/usr/bin/env bash
# Groth16 SETUP for one circuit (run ONCE): compile, lower to R1CS, powers-of-tau,
# zkey, export VK, and a sample proof. Produces a FIXED zkey/VK so multiple notes
# can be proved against one embedded verifier — the structural prerequisite for a
# real ceremony and for OPAQ.md B.8 Tests 4-6.
#
# NOTE: still an insecure test ceremony (no real contributions) — see B.6.
# Usage: groth16-setup.sh <deposit|withdraw> <out_dir> [ptau_power]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
C="$1"; OUT="$2"; POWER="${3:-16}"
NOIR_CLI="$ROOT/tools/noir-groth16/build/target/release/noir-cli"
CDIR="$ROOT/circuits/$C"
ART="$CDIR/target/$C.json"
mkdir -p "$OUT"

[ -x "$NOIR_CLI" ] || "$ROOT/tools/noir-groth16/setup.sh" >/dev/null
( cd "$CDIR" && nargo compile )
# interop gives circuit.r1cs (circuit-only) + a sample witness.wtns
"$NOIR_CLI" interop "$ART" "$CDIR/inputs.json" --out "$OUT/interop" >/dev/null

# Phase-1 ptau. A zero-contribution local ceremony is fast but yields DEGENERATE
# VK IC points (some at infinity); snarkjs + groth16-solana tolerate that, but the
# EVM ecMul precompile rejects off-curve points — so the Solidity verifier
# (Phase 3) needs a non-degenerate VK. Set OPAQ_PTAU=<pre-prepared phase2 ptau>
# (e.g. the real PPoT via ceremony-fetch-ptau.sh) to use a non-degenerate,
# already-prepared phase-1 — also far faster than a real local prepare-phase2.
if [ -n "${OPAQ_PTAU:-}" ]; then
  PTAU_PREPARED="$OPAQ_PTAU"
else
  snarkjs powersoftau new bn128 "$POWER" "$OUT/pot0.ptau" >/dev/null
  snarkjs powersoftau prepare phase2 "$OUT/pot0.ptau" "$OUT/pot.ptau" >/dev/null
  PTAU_PREPARED="$OUT/pot.ptau"
fi
snarkjs groth16 setup "$OUT/interop/circuit.r1cs" "$PTAU_PREPARED" "$OUT/zkey0.zkey" >/dev/null
snarkjs zkey contribute "$OUT/zkey0.zkey" "$OUT/circuit.zkey" \
  --name=opaq -e="opaq deterministic entropy" >/dev/null
snarkjs zkey export verificationkey "$OUT/circuit.zkey" "$OUT/verification_key.json" >/dev/null
# a sample proof so emit_artifacts (which reads proof.json) can run from this dir
snarkjs groth16 prove "$OUT/circuit.zkey" "$OUT/interop/witness.wtns" \
  "$OUT/proof.json" "$OUT/public.json" >/dev/null

echo "setup $C: fixed zkey at $OUT/circuit.zkey"
