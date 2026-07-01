#!/usr/bin/env bash
# M16 / Phase 3 (OPAQ.md B.11 #3): drive the cross-chain burn from the `opaq` CLI
# — deposit -> `opaq burn --submit` — end-to-end on a validator. Verifies the
# SELF-SERVED burn (no relayer, A.8): the nullifier is recorded, NO SPL released,
# NO tree insert, and a double-burn (nullifier reuse) is rejected on-chain.
#
# NOTE: insecure test ceremony (see OPAQ.md B.6) — exercises the CLI/burn flow.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/opaq"
VKMF="$ROOT/crates/groth16-verify/Cargo.toml"
WORK="$(mktemp -d)"
export OPAQ_PASSPHRASE="opaq-m16-test-passphrase"

cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; }
cd "$ROOT"

echo "==> build opaq CLI + mint keypair"
cargo build -q -p opaq-prover
OPAQ="${CARGO_TARGET_DIR:-$ROOT/target}/debug/opaq"
solana-keygen new --no-bip39-passphrase --silent --force -o "$WORK/mint.json" >/dev/null

# Fixed deposit/burn zkeys + VKs (setup is circuit-only, so the CLI's own
# freshly-minted note proves against them).
echo "==> seed witnesses + fixed zkeys/VKs (deposit, burn)"
cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
for c in deposit:14 burn:16; do
  name="${c%%:*}"; pow="${c##*:}"
  "$ROOT/scripts/groth16-setup.sh" "$name" "$WORK/setup_$name" "$pow"
  cargo run -q --manifest-path "$VKMF" --bin emit_artifacts -- "$WORK/setup_$name" "$WORK" >/dev/null
  mv -f "$WORK/vk.rs" "$PROG/src/vk_$name.rs"
done

echo "==> build opaq program (fixed VKs)"
( cd "$PROG" && cargo build-sbf --tools-version v1.54 )
SBF_DEPLOY="${CARGO_TARGET_DIR:-$PROG/target}/deploy"
PROG_KP="$SBF_DEPLOY/opaq-keypair.json"

VPID=""; cleanup() { [ -n "$VPID" ] && kill "$VPID" 2>/dev/null || true; }; trap cleanup EXIT
echo "==> start validator"
solana-test-validator --reset --quiet --ledger "$WORK/ledger" >/tmp/opaq-m16-validator.log 2>&1 &
VPID=$!
solana config set --url http://127.0.0.1:8899 >/dev/null
printf "==> waiting for RPC"
for _ in $(seq 1 60); do solana cluster-version >/dev/null 2>&1 && break; printf "."; sleep 1; done
echo " ready"
[ -f ~/.config/solana/id.json ] || solana-keygen new --no-bip39-passphrase --silent -o ~/.config/solana/id.json >/dev/null
solana airdrop 5 >/dev/null 2>&1 || true

echo "==> deploy opaq"
solana program deploy "$SBF_DEPLOY/opaq.so" --program-id "$PROG_KP"

echo "==> run M16 CLI burn e2e"
OPAQ_DEPOSIT_ZKEY="$WORK/setup_deposit/circuit.zkey" \
OPAQ_BURN_ZKEY="$WORK/setup_burn/circuit.zkey" \
OPAQ_ROOT="$ROOT" \
node "$ROOT/tests/m16_burn_cli.mjs" "$PROG_KP" "$WORK/mint.json" "$OPAQ"
