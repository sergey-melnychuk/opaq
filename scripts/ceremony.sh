#!/usr/bin/env bash
# Run the full trusted-setup ceremony end-to-end and embed the resulting VKs:
#   1. reuse PPoT phase-1 (fetch + verify)            -> ceremony-fetch-ptau.sh
#   2. phase-2 ceremony for every circuit              -> ceremony-phase2.sh
#   3. embed real VKs into programs/opaq/src/vk_*.rs  -> emit_artifacts --real
#   4. write ceremony/transcript.md (auditable record)
#
# All 5 circuits (deposit, withdraw, transfer, burn, xburn), each at its own
# ptau power (see CIRCUITS below) — transfer needs power 17, the rest share
# power 16 (deposit's 14 fits inside 16 fine, same as every other script here).
#
# Two profiles:
#   --smoke           local 2 urandom contributions + drand-latest beacon per
#                     circuit. Proves the pipeline yields VERIFYING proofs and
#                     replaces any fixed/known-entropy zkey with one nobody
#                     could have pre-computed a backdoor for — real improvement
#                     over a trivial setup, but still NOT trustworthy for real
#                     funds (one machine saw all toxic waste; see README's
#                     "What toxic waste is" section for why that's the limit).
#   (default / real)  coordinated: you supply contributors and a PINNED drand
#                     round. See README; this driver runs --smoke unless --real-run
#                     is given with --drand-round and pre-collected contributions.
#
# Usage: ceremony.sh --smoke
#        ceremony.sh --real-run --drand-round <R> --work deposit:<dir> --work withdraw:<dir> ...
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VKMF="$ROOT/crates/groth16-verify/Cargo.toml"
PROG="$ROOT/programs/opaq"
CER="$ROOT/ceremony"

# circuit:power — the power each circuit's R1CS needs (matches every other
# setup script in this repo, e.g. scripts/m13-transfer-cli.sh's "deposit:14
# transfer:17 withdraw:16").
CIRCUITS="deposit:14 withdraw:16 transfer:17 burn:16 xburn:16"

MODE="smoke"; ROUND="latest"; WORK_OVERRIDES=""  # space-separated "circuit:dir" (bash 3.2 has no assoc arrays)
while [ $# -gt 0 ]; do
  case "$1" in
    --smoke) MODE="smoke"; shift ;;
    --real-run) MODE="real"; shift ;;
    --drand-round) ROUND="$2"; shift 2 ;;
    --work) WORK_OVERRIDES="$WORK_OVERRIDES $2"; shift 2 ;;
    *) echo "unknown flag: $1" >&2; exit 1 ;;
  esac
done

WORK="$CER/work"
mkdir -p "$WORK"

# workdir_for <circuit> -> the --work override for that circuit, or $WORK/<circuit>
# by default (what smoke mode always uses).
workdir_for() {
  local c="$1" pair
  for pair in $WORK_OVERRIDES; do
    [ "${pair%%:*}" = "$c" ] && { echo "${pair#*:}"; return; }
  done
  echo "$WORK/$c"
}

echo "### Phase 1: reuse PPoT (power 16 and power 17)"
PTAU16="$CER/.cache/powersOfTau28_hez_final_16.ptau"
PTAU17="$CER/.cache/powersOfTau28_hez_final_17.ptau"
"$ROOT/scripts/ceremony-fetch-ptau.sh" "$PTAU16" 16
"$ROOT/scripts/ceremony-fetch-ptau.sh" "$PTAU17" 17
ptau_for() { [ "$1" = 17 ] && echo "$PTAU17" || echo "$PTAU16"; }

echo "### Phase 2: ${CIRCUITS}"
if [ "$MODE" = smoke ]; then
  for pair in $CIRCUITS; do
    c="${pair%%:*}"; p="${pair##*:}"
    "$ROOT/scripts/ceremony-phase2.sh" local "$c" "$(ptau_for "$p")" "$(workdir_for "$c")" \
      --contributions 2 --drand-round "$ROUND"
  done
else
  for pair in $CIRCUITS; do
    c="${pair%%:*}"
    w="$(workdir_for "$c")"
    [ -n "$WORK_OVERRIDES" ] && echo "$WORK_OVERRIDES" | grep -q "$c:" || { echo "--real-run needs --work $c:<dir> (post-contribution dir)" >&2; exit 1; }
    "$ROOT/scripts/ceremony-phase2.sh" finalize "$w" --drand-round "$ROUND"
  done
fi

echo "### Embed real VKs"
emit_real() {  # circuit work_dir -> programs/opaq/src/vk_<circuit>.rs
  local c="$1" w="$2"
  cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$w" "$w" --real >/dev/null
  mv -f "$w/vk.rs" "$PROG/src/vk_$c.rs"
  echo "    embedded $PROG/src/vk_$c.rs"
}
for pair in $CIRCUITS; do
  c="${pair%%:*}"
  emit_real "$c" "$(workdir_for "$c")"
done

echo "### Transcript"
b2() { command -v b2sum >/dev/null 2>&1 && b2sum "$1" | awk '{print $1}' || echo "(no b2sum)"; }
vkhash() { b2 "$1"; }
{
  echo "# Opaq trusted-setup ceremony transcript"
  echo
  echo "- Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "- Profile: **$MODE**$([ "$MODE" = smoke ] && echo '  ⚠️ NOT trustworthy — tooling smoke only')"
  echo
  echo "## Phase 1 (reused)"
  echo "- power 16: \`powersOfTau28_hez_final_16.ptau\` — blake2b-512 \`6a6277a2f74e1073601b4f9fed6e1e55226917efb0f0db8a07d98ab01df1ccf43eb0e8c3159432acd4960e2f29fe84a4198501fa54c8dad9e43297453efec125\`"
  echo "- power 17: \`powersOfTau28_hez_final_17.ptau\` — blake2b-512 \`6247a3433948b35fbfae414fa5a9355bfb45f56efa7ab4929e669264a0258976741dfbe3288bfb49828e5df02c2e633df38d2245e30162ae7e3bcca5b8b49345\`"
  echo "- Source: https://storage.googleapis.com/zkevm/ptau/"
  echo
  for pair in $CIRCUITS; do
    c="${pair%%:*}"; w="$(workdir_for "$c")"
    echo "## Phase 2: $c"
    echo "- Contributions:"
    if [ -f "$w/contributions.tsv" ]; then
      while IFS=$'\t' read -r name file; do echo "  - $name → \`$file\`"; done < "$w/contributions.tsv"
    fi
    echo "- Final beacon: $(cat "$w/beacon.label" 2>/dev/null) (\`$(cat "$w/beacon.value" 2>/dev/null)\`)"
    echo "- final.zkey blake2b-512: \`$(vkhash "$w/final.zkey")\`"
    echo "- embedded VK (\`programs/opaq/src/vk_$c.rs\`) blake2b-512: \`$(vkhash "$PROG/src/vk_$c.rs")\`"
    echo
  done
  echo "## Verify"
  echo '```'
  echo -n "scripts/ceremony-verify.sh"
  for pair in $CIRCUITS; do c="${pair%%:*}"; echo -n " ${c}:$(workdir_for "$c")"; done
  echo
  echo '```'
} > "$CER/transcript.md"
echo "    wrote $CER/transcript.md"

echo
echo "DONE ($MODE). Embedded VKs + transcript. Run scripts/ceremony-verify.sh to audit."
[ "$MODE" = smoke ] && echo "NOTE: --smoke output is NOT trustworthy. Re-run --real-run before mainnet."
