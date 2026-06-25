#!/usr/bin/env bash
# M0.5 (OPAQ.md B.6): measure real Solana CU per alt_bn128 op + per BN254 Fr
# multiply, and estimate UltraHonk vs Groth16 verifier cost vs the 1.4M ceiling.
# Repeatable; tears down the validator on exit.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/cu-probe"
KEYPAIR="$PROG/target/deploy/cu_probe-keypair.json"
SO="$PROG/target/deploy/cu_probe.so"
LEDGER="$(mktemp -d)/ledger"

VPID=""
cleanup() { [ -n "$VPID" ] && kill "$VPID" 2>/dev/null || true; }
trap cleanup EXIT

echo "==> build cu-probe"
( cd "$PROG" && cargo build-sbf --tools-version v1.54 )

echo "==> start validator"
solana-test-validator --reset --quiet --ledger "$LEDGER" >/tmp/opaq-m05-validator.log 2>&1 &
VPID=$!
solana config set --url http://127.0.0.1:8899 >/dev/null
printf "==> waiting for RPC"
for _ in $(seq 1 60); do solana cluster-version >/dev/null 2>&1 && break; printf "."; sleep 1; done
echo " ready"

[ -f ~/.config/solana/id.json ] || solana-keygen new --no-bip39-passphrase --silent --outfile ~/.config/solana/id.json >/dev/null
solana airdrop 5 >/dev/null 2>&1 || true

echo "==> deploy"
solana program deploy "$SO" --program-id "$KEYPAIR"

echo "==> measure"
( cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; } && node m05_cu.mjs "$KEYPAIR" )
