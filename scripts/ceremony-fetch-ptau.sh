#!/usr/bin/env bash
# Ceremony PHASE 1 (reuse, not generate). Powers-of-Tau is universal — circuit-
# independent — so instead of running our own multi-party phase-1 we reuse the
# Perpetual Powers of Tau (Polygon Hermez snapshot), a large, well-attended BN254
# ceremony already finalized with a public beacon. Power defaults to 16 (64k
# constraints — covers deposit=14, withdraw=16, burn=16, xburn=16); transfer
# needs power 17 (128k), passed as the second arg.
#
# This script downloads the file (if absent), verifies its published blake2b-512
# hash (provenance), and runs `snarkjs powersoftau verify` (cryptographic check
# that the whole contribution chain is a valid PoT). Either gate failing aborts.
#
# Usage: ceremony-fetch-ptau.sh [out_ptau_path] [power]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
POWER="${2:-16}"
FILE="powersOfTau28_hez_final_${POWER}.ptau"
URL="https://storage.googleapis.com/zkevm/ptau/${FILE}"
# Published blake2b-512 hashes (snarkjs README's ptau table / zkevm ptau bucket).
case "$POWER" in
  16) EXPECTED_B2="6a6277a2f74e1073601b4f9fed6e1e55226917efb0f0db8a07d98ab01df1ccf43eb0e8c3159432acd4960e2f29fe84a4198501fa54c8dad9e43297453efec125" ;;
  17) EXPECTED_B2="6247a3433948b35fbfae414fa5a9355bfb45f56efa7ab4929e669264a0258976741dfbe3288bfb49828e5df02c2e633df38d2245e30162ae7e3bcca5b8b49345" ;;
  *) echo "no pinned hash for power $POWER — add one from snarkjs's ptau table before using it" >&2; exit 1 ;;
esac

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
