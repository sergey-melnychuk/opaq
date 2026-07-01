#!/usr/bin/env bash
# Fun demo: on-chain Tamagotchi + r/place (raw native Solana), rendered in the
# terminal. Builds + deploys both throwaway programs on a fresh validator.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"

cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; }
cd "$ROOT"

echo "==> build tamagotchi + rplace (SBF)"
( cd "$ROOT/programs/tamagotchi" && cargo build-sbf --tools-version v1.54 )
( cd "$ROOT/programs/rplace" && cargo build-sbf --tools-version v1.54 )
PET_DEPLOY="${CARGO_TARGET_DIR:-$ROOT/programs/tamagotchi/target}/deploy"
PLACE_DEPLOY="${CARGO_TARGET_DIR:-$ROOT/programs/rplace/target}/deploy"

VPID=""; cleanup() { [ -n "$VPID" ] && kill "$VPID" 2>/dev/null || true; }; trap cleanup EXIT
echo "==> start validator"
solana-test-validator --reset --quiet --ledger "$WORK/ledger" >/tmp/opaq-fun-validator.log 2>&1 &
VPID=$!
solana config set --url http://127.0.0.1:8899 >/dev/null
printf "==> waiting for RPC"
for _ in $(seq 1 60); do solana cluster-version >/dev/null 2>&1 && break; printf "."; sleep 1; done
echo " ready"
[ -f ~/.config/solana/id.json ] || solana-keygen new --no-bip39-passphrase --silent -o ~/.config/solana/id.json >/dev/null
solana airdrop 5 >/dev/null 2>&1 || true

echo "==> deploy both programs"
solana program deploy "$PET_DEPLOY/tamagotchi.so"   --program-id "$PET_DEPLOY/tamagotchi-keypair.json"
solana program deploy "$PLACE_DEPLOY/rplace.so"     --program-id "$PLACE_DEPLOY/rplace-keypair.json"

node "$ROOT/tests/fun_demo.mjs" "$PET_DEPLOY/tamagotchi-keypair.json" "$PLACE_DEPLOY/rplace-keypair.json"
