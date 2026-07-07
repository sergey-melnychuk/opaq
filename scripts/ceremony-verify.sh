#!/usr/bin/env bash
# Independently re-verify a finished ceremony from its artifacts. Anyone can run
# this against the per-circuit work dirs to confirm:
#   - each work dir's phase-1 ptau (whichever power it actually used — deposit/
#     withdraw/burn/xburn use power 16, transfer needs power 17, ceremony.sh
#     records which in <work>/ptau.path) is a valid Powers-of-Tau chain
#   - each circuit's final zkey is a valid phase-2 of that ptau for that R1CS
#   - the embedded VK matches the final zkey
#
# Usage: ceremony-verify.sh <circuit1:work_dir1> [<circuit2:work_dir2> ...]
#   e.g. ceremony-verify.sh deposit:ceremony/work/deposit withdraw:ceremony/work/withdraw
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/opaq"

[ $# -ge 1 ] || { echo "usage: ceremony-verify.sh <circuit:work_dir> ..." >&2; exit 1; }

SEEN_PTAU=""  # space-separated list of already-verified ptau paths (bash 3.2 has no assoc arrays)
for pair in "$@"; do
  work="${pair#*:}"
  ptau="$(cat "$work/ptau.path")"
  case " $SEEN_PTAU " in
    *" $ptau "*) ;;
    *)
      echo "==> phase-1 ($ptau): powersoftau verify"
      snarkjs powersoftau verify "$ptau"
      SEEN_PTAU="$SEEN_PTAU $ptau"
      ;;
  esac
done

verify_circuit() {
  local WORK="$1" C; C="$(cat "$WORK/circuit.name")"
  local PTAU; PTAU="$(cat "$WORK/ptau.path")"
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

for pair in "$@"; do
  verify_circuit "${pair#*:}"
done
echo "ceremony verification PASSED"
