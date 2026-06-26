#!/usr/bin/env bash
# Prove ONE note against an already-fixed zkey (from groth16-setup.sh). Lowers
# the note's inputs to a witness and proves — so every note shares the same VK.
# Usage: groth16-prove-note.sh <deposit|withdraw> <circuit.zkey> <inputs.json> <out_dir>
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
C="$1"; ZKEY="$2"; INPUTS="$3"; OUT="$4"
NOIR_CLI="$ROOT/tools/noir-groth16/build/target/release/noir-cli"
ART="$ROOT/circuits/$C/target/$C.json"
mkdir -p "$OUT"

"$NOIR_CLI" interop "$ART" "$INPUTS" --out "$OUT/interop" >/dev/null
snarkjs groth16 prove "$ZKEY" "$OUT/interop/witness.wtns" \
  "$OUT/proof.json" "$OUT/public.json" >/dev/null
