#!/usr/bin/env bash
# Per OPAQ.md B.1: print all installed toolchain versions. Commit the output
# expectations and re-run any time something inexplicably breaks — version
# drift is the single most likely source of "works on my machine" bugs here.
set -uo pipefail

echo "=== Opaq toolchain versions ($(date -u +%Y-%m-%dT%H:%M:%SZ)) ==="
printf '%-18s %s\n' "rustc"   "$(rustc --version 2>&1)"
printf '%-18s %s\n' "cargo"   "$(cargo --version 2>&1)"
printf '%-18s %s\n' "nargo"   "$(nargo --version 2>&1 | head -1)"
printf '%-18s %s\n' "bb"      "$(bb --version 2>&1)"
printf '%-18s %s\n' "solana"  "$(solana --version 2>&1)"
printf '%-18s %s\n' "node"    "$(node --version 2>&1)"

echo
echo "=== OPAQ.md B.1 pins (compare against the above) ==="
echo "nargo          1.0.0-beta.22"
echo "bb             5.0.0-nightly.20260522 (paired w/ nargo beta.22)"
echo "solana/agave   3.0.15"
echo "SBF platform-tools v1.54 (pass --tools-version v1.54 to cargo build-sbf)"
echo "solana-program 3.x (native program, no framework)"
echo "borsh          1.x"
echo "light-poseidon ^0.4.0"
echo "solana-poseidon 3.x"
echo "ark-bn254      ^0.5.0"
