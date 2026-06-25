#!/usr/bin/env bash
# M0 on-chain leg (OPAQ.md B.0 step 4): deploy poseidon-syscall-check to a fresh
# solana-test-validator and confirm the REAL sol_poseidon syscall matches the
# off-chain reference vectors byte-for-byte. Repeatable; tears down on exit.
#
# Off-chain parity (Noir == light-poseidon == solana-poseidon crate) is covered
# separately by `nargo test` in circuits/poseidon_check and `cargo test -p
# opaq-common`. This script closes the third leg: the syscall in the SBF VM.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/poseidon-syscall-check"
KEYPAIR="$PROG/target/deploy/poseidon_syscall_check-keypair.json"
SO="$PROG/target/deploy/poseidon_syscall_check.so"
LEDGER="$(mktemp -d)/ledger"
TOOLS_VERSION="v1.54"   # cargo 1.89 (edition2024); default v1.51 (cargo 1.84) CANNOT parse the solana 3.x dep graph

VPID=""
cleanup() { [ -n "$VPID" ] && kill "$VPID" 2>/dev/null || true; }
trap cleanup EXIT

echo "==> build SBF program (platform-tools $TOOLS_VERSION)"
( cd "$PROG" && cargo build-sbf --tools-version "$TOOLS_VERSION" )

echo "==> start validator"
solana-test-validator --reset --quiet --ledger "$LEDGER" >/tmp/opaq-m0-validator.log 2>&1 &
VPID=$!
solana config set --url http://127.0.0.1:8899 >/dev/null

printf "==> waiting for RPC"
for _ in $(seq 1 60); do solana cluster-version >/dev/null 2>&1 && break; printf "."; sleep 1; done
echo " ready"

[ -f ~/.config/solana/id.json ] || solana-keygen new --no-bip39-passphrase --silent --outfile ~/.config/solana/id.json >/dev/null
solana airdrop 5 >/dev/null 2>&1 || true

echo "==> deploy"
solana program deploy "$SO" --program-id "$KEYPAIR"

echo "==> run client"
( cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; } && node m0_syscall.mjs "$KEYPAIR" )
