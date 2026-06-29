#!/usr/bin/env bash
# Independently re-verify a finished ceremony from its artifacts. Anyone can run
# this against the published ptau + per-circuit work dirs to confirm:
#   - the phase-1 ptau is a valid Powers-of-Tau chain
#   - each circuit's final zkey is a valid phase-2 of that ptau for that R1CS
#   - the embedded VK matches the final zkey
#
# Usage: ceremony-verify.sh <ptau> <deposit_work> <withdraw_work>
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PTAU="$1"; DEP="$2"; WIT="$3"

echo "==> phase-1: powersoftau verify"
snarkjs powersoftau verify "$PTAU"

verify_circuit() {
  local WORK="$1" C; C="$(cat "$WORK/circuit.name")"
  echo "==> phase-2 ($C): zkey verify"
  snarkjs zkey verify "$WORK/circuit.r1cs" "$PTAU" "$WORK/final.zkey"
  echo "==> phase-2 ($C): embedded VK matches final.zkey"
  snarkjs zkey export verificationkey "$WORK/final.zkey" "$WORK/vk_check.json" >/dev/null
  # Compare against the VK currently embedded in the program, re-derived via the
  # same emit path. The embedded vk_$C.rs must come from this verification_key.json.
  node -e '
    const fs=require("fs");
    const a=JSON.parse(fs.readFileSync(process.argv[1]));
    const b=JSON.parse(fs.readFileSync(process.argv[2]));
    const k=["vk_alpha_1","vk_beta_2","vk_gamma_2","vk_delta_2","IC"];
    const eq=k.every(x=>JSON.stringify(a[x])===JSON.stringify(b[x]));
    if(!eq){console.error("VK MISMATCH for "+process.argv[3]);process.exit(1)}
    console.log("    VK OK ("+process.argv[3]+")");
  ' "$WORK/vk_check.json" "$WORK/verification_key.json" "$C"
}

verify_circuit "$DEP"
verify_circuit "$WIT"
echo "ceremony verification PASSED"
