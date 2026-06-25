#!/usr/bin/env bash
# M1/M2 (OPAQ.md B.9): compile, prove, and verify the deposit & withdraw
# circuits locally with bb (UltraHonk) — no Solana yet. Regenerates the
# Prover.toml fixtures from crates/common first, so the off-chain witness math
# is re-checked against every in-circuit assert. Repeatable.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "==> generate witnesses (Prover.toml) via light-poseidon"
cargo run -q -p opaq-common --bin gen_witness -- circuits

for c in deposit withdraw; do
  echo "==> $c: compile / execute / write_vk / prove / verify"
  (
    cd "circuits/$c"
    nargo compile
    nargo execute "${c}_witness" >/dev/null
    mkdir -p target/bb
    bb write_vk -b "target/$c.json" -o target/bb >/dev/null
    bb prove -b "target/$c.json" -w "target/${c}_witness.gz" -k target/bb/vk -o target/bb >/dev/null
    bb verify -k target/bb/vk -p target/bb/proof -i target/bb/public_inputs
  )
done

echo "==> M1/M2 OK: deposit & withdraw both prove and verify (UltraHonk)"
