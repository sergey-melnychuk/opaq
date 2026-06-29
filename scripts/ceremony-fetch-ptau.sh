#!/usr/bin/env bash
# Ceremony PHASE 1 (reuse, not generate). Powers-of-Tau is universal — circuit-
# independent — so instead of running our own multi-party phase-1 we reuse the
# Perpetual Powers of Tau (Polygon Hermez snapshot), a large, well-attended BN254
# ceremony already finalized with a public beacon. We need <= 2^16 constraints
# (withdraw=16 ⊇ deposit=14), so power 16 (64k) suffices for both circuits.
#
# This script downloads the file (if absent), verifies its published blake2b-512
# hash (provenance), and runs `snarkjs powersoftau verify` (cryptographic check
# that the whole contribution chain is a valid PoT). Either gate failing aborts.
#
# Usage: ceremony-fetch-ptau.sh [out_ptau_path]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
POWER=16
FILE="powersOfTau28_hez_final_${POWER}.ptau"
URL="https://storage.googleapis.com/zkevm/ptau/${FILE}"
# Published blake2b-512 of the power-16 file (snarkjs README / zkevm ptau bucket).
EXPECTED_B2="6a6277a2f74e1073601b4f9fed6e1e55226917efb0f0db8a07d98ab01df1ccf43eb0e8c3159432acd4960e2f29fe84a4198501fa54c8dad9e43297453efec125"

OUT="${1:-$ROOT/ceremony/.cache/$FILE}"
mkdir -p "$(dirname "$OUT")"

blake2b512() {  # echo blake2b-512 hex of $1, or return 1 if no tool available
  if command -v b2sum >/dev/null 2>&1; then
    b2sum "$1" | awk '{print $1}'
  elif openssl dgst -blake2b512 "$1" >/dev/null 2>&1; then
    openssl dgst -blake2b512 "$1" | awk '{print $NF}'
  else
    return 1
  fi
}

if [ ! -f "$OUT" ]; then
  echo "==> downloading $FILE (~72 MB) from zkevm ptau bucket"
  curl -fSL --retry 3 -o "$OUT" "$URL"
else
  echo "==> using cached $OUT"
fi

echo "==> provenance: blake2b-512 vs published hash"
if GOT="$(blake2b512 "$OUT")"; then
  if [ "$GOT" = "$EXPECTED_B2" ]; then
    echo "    OK  $GOT"
  else
    echo "    HASH MISMATCH" >&2
    echo "    got      $GOT" >&2
    echo "    expected $EXPECTED_B2" >&2
    echo "    Refusing to use a ptau whose hash does not match the published value." >&2
    exit 1
  fi
else
  echo "    SKIP: no blake2b tool (install coreutils for b2sum). Expected:" >&2
  echo "    $EXPECTED_B2" >&2
  echo "    Relying on 'powersoftau verify' below for the cryptographic check." >&2
fi

echo "==> cryptographic: snarkjs powersoftau verify (validates the whole chain)"
snarkjs powersoftau verify "$OUT"

echo "ptau OK: $OUT"
