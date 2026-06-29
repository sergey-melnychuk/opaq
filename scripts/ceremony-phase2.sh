#!/usr/bin/env bash
# Ceremony PHASE 2 (per-circuit, must be run for OUR circuits — phase-2 is bound
# to a specific R1CS, so nothing can be inherited from another project's ceremony).
# Groth16 phase-2 needs >=1 honest contributor who destroys their toxic waste,
# then a public unpredictable beacon to finalize.
#
# Modes:
#   init     <deposit|withdraw> <ptau> <work>   compile+lower+groth16 setup -> 0000.zkey
#   contribute <work> <name>                    one party's contribution (handoff)
#   finalize <work> [--beacon ...]              beacon + zkey verify + export VK
#   local    <deposit|withdraw> <ptau> <work>   init + N urandom contributions + finalize
#                                               (--contributions N, for testing/solo)
#
# Beacon flags (finalize / local):
#   --beacon-source drand   --drand-round <R|latest>   (default; verifiable, scriptable)
#   --beacon-source hex     --beacon-value <64-hex>    (escape hatch: block hash, etc.)
#
# DISTRIBUTED USE: coordinator runs `init`, publishes work/; each party runs
# `contribute` in turn (snarkjs prompts for entropy when -e is omitted) and hands
# back; coordinator runs `finalize` with a drand round PINNED IN ADVANCE.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NOIR_CLI="$ROOT/tools/noir-groth16/build/target/release/noir-cli"

# ---- beacon resolution -------------------------------------------------------
resolve_beacon() {  # args: source value round  -> echoes "<hex64> <label>"
  local source="$1" value="$2" round="$3"
  case "$source" in
    hex)
      [ ${#value} -eq 64 ] || { echo "beacon hex must be 64 hex chars" >&2; return 1; }
      echo "$value hex"
      ;;
    drand)
      local url
      if [ -z "$round" ] || [ "$round" = latest ]; then
        url="https://api.drand.sh/public/latest"
      else
        url="https://api.drand.sh/public/$round"
      fi
      local json
      json="$(curl -fsS "$url")" || { echo "drand fetch failed ($url)" >&2; return 1; }
      local rnd rno
      rnd="$(printf '%s' "$json" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>process.stdout.write(JSON.parse(s).randomness))')"
      rno="$(printf '%s' "$json" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>process.stdout.write(String(JSON.parse(s).round)))')"
      [ ${#rnd} -eq 64 ] || { echo "drand randomness not 64 hex" >&2; return 1; }
      echo "$rnd drand:round=$rno"
      ;;
    *) echo "unknown beacon source: $source" >&2; return 1 ;;
  esac
}

# ---- modes -------------------------------------------------------------------
do_init() {
  local C="$1" PTAU="$2" WORK="$3"
  local CDIR="$ROOT/circuits/$C" ART="$ROOT/circuits/$C/target/$C.json"
  mkdir -p "$WORK"
  [ -x "$NOIR_CLI" ] || "$ROOT/tools/noir-groth16/setup.sh" >/dev/null
  ( cd "$CDIR" && nargo compile )
  # interop -> circuit.r1cs (+ a sample witness.wtns for the finalize proof)
  "$NOIR_CLI" interop "$ART" "$CDIR/inputs.json" --out "$WORK/interop" >/dev/null
  cp "$WORK/interop/circuit.r1cs" "$WORK/circuit.r1cs"
  cp "$WORK/interop/witness.wtns" "$WORK/witness.wtns"
  snarkjs groth16 setup "$WORK/circuit.r1cs" "$PTAU" "$WORK/c_0000.zkey" >/dev/null
  echo "$C" > "$WORK/circuit.name"
  echo "0" > "$WORK/count"
  echo "init $C: $WORK/c_0000.zkey (contribute next, then finalize)"
}

latest_zkey() {  # echoes path to the highest-numbered c_NNNN.zkey in $1
  ls "$1"/c_[0-9][0-9][0-9][0-9].zkey 2>/dev/null | sort | tail -1
}

do_contribute() {
  local WORK="$1" NAME="${2:-anon}" ENTROPY="${3:-}"
  local n prev next
  n="$(cat "$WORK/count")"; n=$((n + 1))
  prev="$(latest_zkey "$WORK")"
  next="$(printf '%s/c_%04d.zkey' "$WORK" "$n")"
  if [ -n "$ENTROPY" ]; then
    snarkjs zkey contribute "$prev" "$next" --name="$NAME" -e="$ENTROPY" >/dev/null
  else
    # no -e: snarkjs prompts the contributor for entropy interactively
    snarkjs zkey contribute "$prev" "$next" --name="$NAME"
  fi
  echo "$n" > "$WORK/count"
  printf '%s\t%s\n' "$NAME" "$(basename "$next")" >> "$WORK/contributions.tsv"
  echo "contribution #$n by '$NAME' -> $(basename "$next")"
}

do_finalize() {
  local WORK="$1"; shift
  local SOURCE=drand VALUE="" ROUND="latest"
  while [ $# -gt 0 ]; do
    case "$1" in
      --beacon-source) SOURCE="$2"; shift 2 ;;
      --beacon-value)  VALUE="$2";  shift 2 ;;
      --drand-round)   ROUND="$2";  shift 2 ;;
      *) echo "unknown finalize flag: $1" >&2; return 1 ;;
    esac
  done
  local C PTAU last beacon hexv label
  C="$(cat "$WORK/circuit.name")"
  PTAU="$(cat "$WORK/ptau.path")"
  last="$(latest_zkey "$WORK")"
  beacon="$(resolve_beacon "$SOURCE" "$VALUE" "$ROUND")" || return 1
  hexv="${beacon%% *}"; label="${beacon#* }"
  echo "==> beacon: $label ($hexv)"
  snarkjs zkey beacon "$last" "$WORK/final.zkey" "$hexv" 10 -n="Final beacon: $label" >/dev/null
  echo "==> zkey verify (r1cs + ptau + final zkey)"
  snarkjs zkey verify "$WORK/circuit.r1cs" "$PTAU" "$WORK/final.zkey"
  snarkjs zkey export verificationkey "$WORK/final.zkey" "$WORK/verification_key.json" >/dev/null
  # sample proof so emit_artifacts (reads proof.json/public.json) can run
  snarkjs groth16 prove "$WORK/final.zkey" "$WORK/witness.wtns" \
    "$WORK/proof.json" "$WORK/public.json" >/dev/null
  printf '%s' "$label" > "$WORK/beacon.label"
  printf '%s' "$hexv"  > "$WORK/beacon.value"
  echo "finalized $C: $WORK/final.zkey + verification_key.json"
}

do_local() {
  local C="$1" PTAU="$2" WORK="$3"; shift 3
  local N=2 SOURCE=drand VALUE="" ROUND="latest"
  while [ $# -gt 0 ]; do
    case "$1" in
      --contributions) N="$2"; shift 2 ;;
      --beacon-source) SOURCE="$2"; shift 2 ;;
      --beacon-value)  VALUE="$2";  shift 2 ;;
      --drand-round)   ROUND="$2";  shift 2 ;;
      *) echo "unknown local flag: $1" >&2; return 1 ;;
    esac
  done
  do_init "$C" "$PTAU" "$WORK"
  printf '%s' "$PTAU" > "$WORK/ptau.path"
  local i
  for i in $(seq 1 "$N"); do
    do_contribute "$WORK" "local-$i" "$(head -c 64 /dev/urandom | xxd -p | tr -d '\n')"
  done
  do_finalize "$WORK" --beacon-source "$SOURCE" --beacon-value "$VALUE" --drand-round "$ROUND"
}

# ---- dispatch ----------------------------------------------------------------
MODE="${1:-}"; shift || true
case "$MODE" in
  init)       do_init "$@"; printf '%s' "$2" > "$3/ptau.path" ;;
  contribute) do_contribute "$@" ;;
  finalize)   do_finalize "$@" ;;
  local)      do_local "$@" ;;
  *) echo "usage: ceremony-phase2.sh {init|contribute|finalize|local} ..." >&2; exit 1 ;;
esac
