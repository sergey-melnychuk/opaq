# Opaq — a privacy pool for Solana

> *Opaque from outside, clear from inside.*

Opaq is a Solana-native **privacy pool** for SPL tokens: shield a deposit behind
a zero-knowledge commitment, then withdraw to a fresh address with no on-chain
link to the deposit. UTXO model (commitments + nullifiers), a single shared
Poseidon Merkle tree, Groth16 proofs over BN254, and a **bare native** Solana
program (no Anchor).

**Phase 1 works end-to-end on a validator** — a real shielded deposit →
withdraw round-trip, with double-spend prevention, amount/recipient binding,
stale-root tolerance, multi-token isolation, and a zero-infrastructure read
path (no indexer). See [`OPAQ.md`](OPAQ.md) for the full design + build spec.

> ⚠️ **Not for real funds.** The proving uses an **insecure test ceremony**
> (trivial powers-of-tau → forgeable verifying key), and the Noir→Groth16
> backend is unaudited. A real ceremony + audit are required before deployment.
> See `OPAQ.md` §B.6.

## How it works

```
Noir circuit (deposit.nr / withdraw.nr)
  → ACIR → R1CS            (vendored Noir→Groth16 backend, tools/noir-groth16)
  → Groth16 proof (BN254)  (snarkjs)
  → native Solana program  (verifies via groth16-solana / alt_bn128 syscalls)
       deposit:  verify + bind (token_id, amount, commitment) + SPL→vault + Merkle insert
       withdraw: verify + recent-root check + nullifier + SPL vault→recipient
```

The whole pool fits one transaction's compute budget because Phase 1 uses
**Groth16** (~272k CU verify), not UltraHonk (measured 3–12× over budget — see
the M0.5 spike in `OPAQ.md` §B.6).

## Quickstart

**Prereqs:** Rust 1.95, `nargo` 1.0.0-beta.22, `bb` 5.0-nightly, Solana/Agave
3.0.15 + SBF platform-tools **v1.54** (`cargo build-sbf --tools-version v1.54`),
Node ≥ 20, `snarkjs`. Run `scripts/check-versions.sh` to sanity-check.

```bash
# Poseidon parity gate (circuit == light-poseidon == on-chain syscall)
nargo test --show-output -p poseidon_check    # in circuits/poseidon_check
cargo test -p opaq-common                      # tree, nullifier, hash parity

# Circuits prove & verify locally (Groth16)
./scripts/groth16-setup.sh deposit /tmp/dep 14 && ./scripts/groth16-prove-note.sh ...

# Full deposit→withdraw round-trip + all negative tests, on a fresh validator
./scripts/m8-e2e.sh

# Zero-infra read path: reconstruct a Merkle path from RPC only, then withdraw
./scripts/m10-zero-infra.sh
```

`scripts/m8-e2e.sh` is the headline: it mints an SPL token, deposits two notes
(+ a second token), withdraws against a stale root, and asserts that forged
amounts, wrong recipients, and double-spends are all rejected on-chain.

## The `opaq` CLI (note management)

```bash
export OPAQ_PASSPHRASE=...                      # encrypts the note at rest
opaq deposit  --token <mint> --amount 1000 --note note.json [--inputs-out in.json]
opaq withdraw --note note.json --recipient <pubkey> [--leaves leaves.json --inputs-out wd.json]
```

`deposit` generates fresh secrets, derives the commitment, writes an **encrypted**
note (Argon2id + ChaCha20-Poly1305), and prints the public inputs (+ privacy
warnings, A.8/A.12). `withdraw` decrypts the note, derives the nullifier, and —
given the RPC-harvested leaf list — rebuilds the Merkle path locally.

## Layout

| path | what |
|------|------|
| `circuits/` | Noir circuits: `deposit`, `withdraw` (+ `poseidon_check` spike) |
| `crates/common` | shared Poseidon/`to_field`/Merkle + nullifier logic (host-tested) |
| `crates/prover` | the `opaq` CLI (encrypted note lifecycle, warnings) |
| `crates/groth16-verify` | snarkjs→`groth16-solana` byte conversion + verify |
| `programs/opaq` | the native pool program (initialize / deposit / withdraw) |
| `programs/*-check` | throwaway probes (Poseidon syscall, BN254 CU, Groth16 verify) |
| `tools/noir-groth16` | vendored Noir→Groth16 backend, ported to nargo beta.22 |
| `scripts/` | build/prove/e2e drivers (`m0`–`m11`, `groth16-*`) |
| `tests/` | JS e2e harnesses (validator + SPL + RPC read path) |
| `OPAQ.md` | design rationale + build spec + milestone checklist |

## Status

Milestones M0–M11 (`OPAQ.md` §B.9) are green: Poseidon parity, circuits,
Groth16-vs-UltraHonk decision, on-chain verifier, tree/nullifier, the full
program, end-to-end tests (round-trip, double-spend, forged-input, stale-root,
multi-token), the prover CLI, the zero-infra read path, and a devnet demo.

**Remaining before real use:** a secure multi-party ceremony, an audit of the
Noir→Groth16 backend, Phase 2 (private join-split transfers, hidden amounts),
and Phase 3 (cross-chain burn/mint).

## Sidenote: on-chain toys

Two throwaway native-Solana programs sit under `programs/` as playful demos of
the same bare-metal, no-Anchor style the pool uses — unrelated to Opaq, just for
fun: `tamagotchi` (an on-chain pet whose stats decay against the `Clock` slot)
and `rplace` (a shared 32×32 pixel canvas). `scripts/fun-demo.sh` builds both,
starts a validator, and renders them in the terminal.
