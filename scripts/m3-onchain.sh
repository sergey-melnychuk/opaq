#!/usr/bin/env bash
# M3 on-chain leg (OPAQ.md B.6): regenerate a deposit Groth16 proof + VK, embed
# the VK in the verifier program, deploy to a fresh solana-test-validator, and
# confirm sol_alt_bn128 verifies the proof (and rejects a tampered one).
# Repeatable; tears down the validator on exit.
#
# NOTE: insecure test ceremony (forgeable vk) — verifier mechanics only. See B.6.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROG="$ROOT/programs/groth16-verify-check"
KEYPAIR="$PROG/target/deploy/groth16_verify_check-keypair.json"
SO="$PROG/target/deploy/groth16_verify_check.so"
GROTH="$ROOT/tools/noir-groth16/build/target/groth16/proof"
LEDGER="$(mktemp -d)/ledger"

echo "==> regenerate deposit proof + vk"
"$ROOT/scripts/groth16-prove.sh" "$ROOT/circuits/deposit" >/dev/null

echo "==> emit vk.rs + instruction.bin"
mkdir -p "$PROG/fixtures"
cargo run -q --manifest-path "$ROOT/crates/groth16-verify/Cargo.toml" --bin emit_artifacts -- \
  "$GROTH" "$PROG/fixtures"
mv -f "$PROG/fixtures/vk.rs" "$PROG/src/vk.rs"

echo "==> build SBF program"
( cd "$PROG" && cargo build-sbf --tools-version v1.54 )

VPID=""
cleanup() { [ -n "$VPID" ] && kill "$VPID" 2>/dev/null || true; }
trap cleanup EXIT

echo "==> start validator"
solana-test-validator --reset --quiet --ledger "$LEDGER" >/tmp/opaq-m3-validator.log 2>&1 &
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
( cd "$ROOT/tests" && { [ -d node_modules ] || npm install --silent; } && \
    node m3_onchain.mjs "$KEYPAIR" "$PROG/fixtures/instruction.bin" )
