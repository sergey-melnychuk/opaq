#!/usr/bin/env bash
# M21 / Phase 2.5 P2.5.2 (OPAQ.md B.13): drive the viewing-key note-discovery
# loop end-to-end on a validator — Alice deposits + transfers to Bob's
# published meta-address (opaq transfer --to-view), Bob discovers the note
# with `opaq list-unspent` using ONLY his identity file (zero out-of-band
# handoff), then withdraws it. This is B.13's closing accept criterion.
#
# NOTE: insecure test ceremony (see OPAQ.md B.6) — exercises the CLI/discovery flow.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/opaq"
VKMF="$ROOT/crates/groth16-verify/Cargo.toml"
WORK="$(mktemp -d)"

cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; }
cd "$ROOT"

echo "==> build opaq CLI + mint keypair"
cargo build -q -p opaq-prover
OPAQ="${CARGO_TARGET_DIR:-$ROOT/target}/debug/opaq"
solana-keygen new --no-bip39-passphrase --silent --force -o "$WORK/mint.json" >/dev/null

echo "==> seed witnesses + fixed zkeys/VKs (deposit, transfer, withdraw)"
cargo run -q -p opaq-common --bin gen_witness -- "$ROOT/circuits" >/dev/null
for c in deposit:14 transfer:17 withdraw:16; do
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
solana-test-validator --reset --quiet --ledger "$WORK/ledger" >/tmp/opaq-m21-validator.log 2>&1 &
VPID=$!
solana config set --url http://127.0.0.1:8899 >/dev/null
printf "==> waiting for RPC"
for _ in $(seq 1 60); do solana cluster-version >/dev/null 2>&1 && break; printf "."; sleep 1; done
echo " ready"
[ -f ~/.config/solana/id.json ] || solana-keygen new --no-bip39-passphrase --silent -o ~/.config/solana/id.json >/dev/null
solana airdrop 5 >/dev/null 2>&1 || true

echo "==> deploy opaq"
solana program deploy "$SBF_DEPLOY/opaq.so" --program-id "$PROG_KP"

echo "==> run M21 view-key discovery e2e"
OPAQ_DEPOSIT_ZKEY="$WORK/setup_deposit/circuit.zkey" \
OPAQ_TRANSFER_ZKEY="$WORK/setup_transfer/circuit.zkey" \
OPAQ_WITHDRAW_ZKEY="$WORK/setup_withdraw/circuit.zkey" \
OPAQ_ROOT="$ROOT" \
node "$ROOT/tests/m21_view_key_discovery.mjs" "$PROG_KP" "$WORK/mint.json" "$OPAQ"
