#!/usr/bin/env bash
# Run the full trusted-setup ceremony end-to-end and embed the resulting VKs:
#   1. reuse PPoT phase-1 (fetch + verify)            -> ceremony-fetch-ptau.sh
#   2. phase-2 ceremony for deposit + withdraw        -> ceremony-phase2.sh
#   3. embed real VKs into programs/opaq/src/vk_*.rs  -> emit_artifacts --real
#   4. write ceremony/transcript.md (auditable record)
#
# Two profiles:
#   --smoke           local 2 urandom contributions + drand-latest beacon. Proves
#                     the pipeline yields VERIFYING proofs. NOT trustworthy (one
#                     machine saw all toxic waste) — for testing the tooling only.
#   (default / real)  coordinated: you supply contributors and a PINNED drand
#                     round. See README; this driver runs --smoke unless --real-run
#                     is given with --drand-round and pre-collected contributions.
#
# Usage: ceremony.sh --smoke
#        ceremony.sh --real-run --drand-round <R> --deposit-work <dir> --withdraw-work <dir>
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VKMF="$ROOT/crates/groth16-verify/Cargo.toml"
PROG="$ROOT/programs/opaq"
CER="$ROOT/ceremony"

MODE="smoke"; ROUND="latest"; DEPW=""; WITW=""
while [ $# -gt 0 ]; do
  case "$1" in
    --smoke) MODE="smoke"; shift ;;
    --real-run) MODE="real"; shift ;;
    --drand-round) ROUND="$2"; shift 2 ;;
    --deposit-work) DEPW="$2"; shift 2 ;;
    --withdraw-work) WITW="$2"; shift 2 ;;
    *) echo "unknown flag: $1" >&2; exit 1 ;;
  esac
done

PTAU="$CER/.cache/powersOfTau28_hez_final_16.ptau"
WORK="$CER/work"
mkdir -p "$WORK"

echo "### Phase 1: reuse PPoT"
"$ROOT/scripts/ceremony-fetch-ptau.sh" "$PTAU"

echo "### Phase 2: deposit + withdraw"
if [ "$MODE" = smoke ]; then
  DEPW="$WORK/deposit"; WITW="$WORK/withdraw"
  "$ROOT/scripts/ceremony-phase2.sh" local deposit  "$PTAU" "$DEPW" --contributions 2 --drand-round "$ROUND"
  "$ROOT/scripts/ceremony-phase2.sh" local withdraw "$PTAU" "$WITW" --contributions 2 --drand-round "$ROUND"
else
  [ -n "$DEPW" ] && [ -n "$WITW" ] || { echo "--real-run needs --deposit-work and --withdraw-work (post-contribution dirs)" >&2; exit 1; }
  "$ROOT/scripts/ceremony-phase2.sh" finalize "$DEPW" --drand-round "$ROUND"
  "$ROOT/scripts/ceremony-phase2.sh" finalize "$WITW" --drand-round "$ROUND"
fi

echo "### Embed real VKs"
emit_real() {  # circuit work_dir -> programs/opaq/src/vk_<circuit>.rs
  local c="$1" w="$2"
  cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$w" "$w" --real >/dev/null
  mv -f "$w/vk.rs" "$PROG/src/vk_$c.rs"
  echo "    embedded $PROG/src/vk_$c.rs"
}
emit_real deposit  "$DEPW"
emit_real withdraw "$WITW"

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
  echo "- File: \`powersOfTau28_hez_final_16.ptau\` (Perpetual Powers of Tau / Hermez, power 16, 64k constraints)"
  echo "- Source: https://storage.googleapis.com/zkevm/ptau/powersOfTau28_hez_final_16.ptau"
  echo "- blake2b-512: \`6a6277a2f74e1073601b4f9fed6e1e55226917efb0f0db8a07d98ab01df1ccf43eb0e8c3159432acd4960e2f29fe84a4198501fa54c8dad9e43297453efec125\`"
  echo
  for pair in "deposit:$DEPW" "withdraw:$WITW"; do
    c="${pair%%:*}"; w="${pair#*:}"
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
  echo "scripts/ceremony-verify.sh \\"
  echo "  ceremony/.cache/powersOfTau28_hez_final_16.ptau \\"
  echo "  $DEPW $WITW"
  echo '```'
} > "$CER/transcript.md"
echo "    wrote $CER/transcript.md"

echo
echo "DONE ($MODE). Embedded VKs + transcript. Run scripts/ceremony-verify.sh to audit."
[ "$MODE" = smoke ] && echo "NOTE: --smoke output is NOT trustworthy. Re-run --real-run before mainnet."
