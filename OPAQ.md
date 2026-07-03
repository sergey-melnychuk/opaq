# Opaq — Multi-Asset Privacy Pool for Solana (UTXO Model, Cross-Chain Ready)

> "Opaque from outside, clear from inside."

Opaq is a Solana-native privacy pool supporting deposit, withdraw, and private
transfer of arbitrary SPL tokens, using a UTXO commitment model. Designed from
the start so that the same circuit/proof primitives can extend to cross-chain
burn/mint (EVM-compatible chains) and later to private lending.

This document is both the design rationale and the build-ready implementation
spec — intended to be handed to an architecture/implementation agent with no
further design discussion needed for Phase 1.

---

## Part A — Design

### A.1 Core Design Choices

| Decision | Choice | Why |
|----------|--------|-----|
| Value model | UTXO (commitments + nullifiers) | Multi-asset, arbitrary splitting, standard privacy-pool pattern (Tornado/Zcash/Railgun lineage) |
| Asset scope | Any SPL token, token-id embedded in commitment | One shared pool instead of one pool per token |
| Tree structure | Single shared incremental Merkle tree | Maximizes anonymity set across all assets |
| Hash function | Poseidon (original, not Poseidon2 — see A.7) | Solana has a native Poseidon syscall — cheap in-circuit and cheap on-chain |
| Curve | BN254 (alt_bn128) | Solana has native BN254 precompiles (add/mul/pairing); same curve Ethereum precompiles use; same curve Noir's default backend (Barretenberg) proves over |
| Circuit language | Noir | Readable, mature Solidity verifier generation (`bb`), BN254-native |
| Cross-chain target | EVM chains | Shares the same curve/verifier story as the Solana side |

The BN254 commonality across Solana syscalls, Ethereum precompiles, and Noir's
backend is the load-bearing design decision — it's what makes the cross-chain
extension realistic instead of theoretical.

> **Caveat on "maximizes anonymity set":** the shared tree maximizes the *leaf*
> anonymity set, but Phase 1 `deposit` and `withdraw` both expose `amount` and
> `token_id` publicly, so the *effective* set for any withdrawal is only the
> deposits sharing its exact `(token_id, amount)`. This is a real, named
> limitation — see A.12.

### A.2 UTXO Model

**Commitment (note):**
```
commitment = Poseidon(token_id, amount, owner_pubkey, blinding_factor)
```
- `token_id` — the SPL mint, embedded directly so one tree serves all tokens. A 32-byte mint does **not** fit in a BN254 field element (~254 bits), so it is encoded as `token_id_field = to_field(mint)` — the canonical two-limb Poseidon encoding defined in B.4.2 — **not** stuffed raw into a `Field`. The off-chain prover and the on-chain program compute this identically.
- `blinding_factor` — random value preventing commitment correlation

**Nullifier:**
```
nullifier = Poseidon(commitment, spend_key)
```
Revealed on spend, recorded in a nullifier set, prevents double-spend without revealing which commitment was spent.

**Note lifecycle:**
```
Deposit:   SPL token → vault PDA, new commitment inserted into tree (amount/token public)
Transfer:  N input commitments spent (nullified) → M output commitments created (join-split, fully private)
Withdraw:  1+ input commitments spent → SPL token released from vault to a public address
```

### A.3 Architecture

```
Noir Circuits (deposit / withdraw / transfer)
       │
       │ nargo compile
       ▼
Off-chain Prover (Rust CLI)
       │  builds witness: merkle path, nullifier, blinding factors
       │  calls Barretenberg (bb) to generate proof
       ▼
UltraHonk/UltraPlonk Proof (BN254)
       │
       │ submit transaction
       ▼
Solana Program (native solana-program)
       │  verifies proof via alt_bn128 syscalls
       │  checks nullifier not already spent
       │  checks merkle root matches current tree state
       │  inserts new commitment(s), updates tree, records nullifier(s)
       ▼
Vault PDA (holds actual SPL tokens) + Commitment Tree PDA + Nullifier Set PDA
```

### A.4 Components Overview

**1. Noir Circuits (`circuits/`)** — `deposit.nr`, `withdraw.nr` in Phase 1; `transfer.nr` in Phase 2. Full specs in Part B.

**2. Off-chain Prover (`crates/prover`)** — Rust CLI wrapping `nargo`/`bb` calls plus Solana RPC interaction. Full spec in Part B.

**3. Solana Program (`programs/opaq`)** — a **barebone native Solana program** (`solana-program` + `borsh` only) holding the vaults, commitment tree, and nullifier set. Instructions are dispatched on the first byte of instruction data (0 = `initialize_pool`, 1 = `deposit`, 2 = `withdraw`, 3 = `transfer`, 4 = `burn`); accounts are PDAs holding borsh-serialized state, parsed/written by hand. Full spec in Part B.

**4. BN254 Verifier** — the piece that doesn't exist off-the-shelf yet: proof verification using Solana's native `alt_bn128` syscalls for the pairing checks, called directly from the native program (via the `groth16-solana` crate — see B.6's resolution). Full spec in Part B, Section B.6.

### A.5 Repository Structure

```
opaq/
├── Cargo.toml
├── rust-toolchain.toml
├── circuits/
│   ├── deposit/
│   ├── withdraw/
│   └── transfer/             # Phase 2
├── crates/
│   ├── prover/                # CLI
│   └── common/                # shared types, note encoding
├── programs/
│   └── opaq/
│       ├── src/lib.rs
│       ├── src/verifier.rs    # BN254/UltraHonk verifier
│       └── src/tree.rs        # incremental Poseidon Merkle tree
├── tests/
│   └── integration.ts
└── OPAQ.md
```

### A.6 Development Phases

**Phase 1 — Single-Asset Deposit/Withdraw (MVP)**
Goal: prove the full pipeline — Noir circuit, BN254 verifier, Poseidon tree, nullifier set — works end-to-end for one token, single input/output. Full detail in Part B and the milestone checklist (B.9).

**Phase 2 — Multi-Asset + Private Transfer (join-split)**
- Generalize tree/vault to handle arbitrary token mints via `token_id` in commitment
- `transfer.nr` — N-input, M-output join-split circuit
- Value conservation constraint per `token_id`
- Note management in CLI (splitting, merging, change outputs)

**Phase 3 — Cross-Chain Burn/Mint (EVM)**
Goal: burn a note on Solana, mint the equivalent asset on an EVM chain, proof-gated.
- `burn.nr` — like withdraw, but output is a public commitment encoding `(token_id, amount, dest_chain, dest_address, nullifier)` instead of releasing SPL tokens
- Relayer pattern (permissionless, proof-gated) — relay the burn-commitment + proof to the EVM side
- Solidity verifier (generated turnkey via `bb`) deployed on the EVM chain
- EVM mint contract: verifies proof, checks nullifier not already used (on EVM side), mints
- Reverse direction (EVM burn → Solana mint) — mirrors Phase 1 verifier work, using the same BN254 verifier already built for the pool itself

**Key invariant:** the same nullifier must be checked on both chains independently — burning on Solana and minting on EVM are two separate proof verifications, not one shared state, since the chains don't share a state root.

**Phase 4 — Symmetric Cross-Chain Shielded Bridge + Advanced Features**
- **Reverse direction as a symmetric bridge (fully spec'd — see B.12):** make the
  EVM side a full shielded pool (Tornado-style Poseidon Merkle tree + nullifier
  set) so cross-chain movement is a private note-burn on the source → note-mint
  (re-shield) on the destination — one Groth16 proof verified on the destination,
  identical machinery both ways. Supersedes the `OpaqMint` balance ledger.
- Lending / advanced (exploratory, not scoped): shielded collateral (prove note
  ownership to back a loan); private interest accrual (time-locked notes).

### A.7 Critical Integration Risk: Poseidon Variant Mismatch

There is one risk that, if wrong, invalidates all circuit work done
afterward — resolve it first, as a standalone spike, before any pool logic.

Noir's standard library (`std::hash`) defaults to **Poseidon2** for general
hashing. Solana's native `sol_poseidon` syscall (exposed via the
`solana-poseidon` crate) implements **original Poseidon** with Circom-style
BN254 parameters (x^5 S-box, via the audited `light-poseidon` crate). These
are different algorithms with different outputs for the same input — if the
circuit hashes with Poseidon2 and the Solana program checks against
original-Poseidon-computed roots/nullifiers, nothing will ever match.

**Resolution:** use the separate `noir-lang/poseidon` library (not
`std::hash::poseidon2_*`) in the circuit, specifically its
`poseidon::bn254::hash_2` / `hash_3` / etc. functions, which implement
original Poseidon with Circom-compatible parameters — the same family
`light-poseidon` implements.

This is the first concrete acceptance gate of the build — see Part B, Section
B.0.

### A.8 Zero-Infra Constraint

Opaq targets zero infrastructure beyond the smart contracts themselves — no
hosted relayer service, no custom indexer, no required backend. The CLI/app
tooling absorbs the UX complexity that infra would otherwise hide.

**What this looks like piece by piece:**

| Concern | Zero-infra approach |
|---------|---------------------|
| Proof generation | Runs entirely client-side (CLI now, WASM-in-browser later) — same model as Tornado Cash and Railgun clients |
| Tree / nullifier state reads | Plain `getAccountInfo` against any public Solana RPC, parsed client-side — feasible because both tree and nullifier set are single PDAs, not scattered across thousands of accounts requiring a custom indexer |
| Cross-chain relay (Phase 3) | Permissionless and self-served — user submits both the burn and the mint transaction themselves; no dedicated relayer service required |

**The one real tension — funding a fresh withdrawal address:**

A freshly generated, never-used recipient address needs a small amount of SOL
to pay its own withdraw transaction fee. With no relayer network (Tornado
Cash's relayers exist specifically to solve this — they pay gas, take a fee,
the withdrawer never has to touch the source funds), this becomes the user's
own operational responsibility. Funding a fresh address carelessly (e.g.
directly from a CEX withdrawal or an already-linked wallet) can reintroduce
exactly the kind of correlation the pool is meant to prevent. This doesn't
disappear by going zero-infra — it moves from "solved architecturally" to
"the CLI must actively warn the user about it." Treat as a real, named risk
rather than glossing over it.

**Recipient flexibility:**

`withdraw`'s `recipient` is a public input the user freely chooses — any
address, not forced to be a fresh one. This is intentional and matches
Tornado Cash / Railgun precedent: restricting withdrawal to only fresh
addresses would be the protocol overriding a decision that belongs to the
user. The risk lives in *silent* misuse, not the flexibility itself — the CLI
should flag when a chosen recipient has prior on-chain history or otherwise
looks linkable (e.g. "this address has existing transaction history —
consider a fresh address for stronger privacy"), rather than the contract
enforcing anything. Education at the tooling layer, not restriction at the
protocol layer.

### A.9 Trust Model

| Moment | Trust required? | In whom |
|--------|----------------|---------|
| Deposit | No | SPL transfer is a normal on-chain transaction |
| Withdraw / Transfer | No | Proof verified on-chain, nullifier enforced on-chain |
| Tree root consistency | No | Enforced by the program itself |
| Cross-chain burn/mint (Phase 3) | Yes, on the bridge oracle — see the ladder below | The two chains don't share consensus, so something must attest Solana burns to the EVM mint |

Phase 1 and 2 are fully trustless by construction. Phase 3's Solana side is too
(the `burn` instruction verifies the proof + a valid root + the nullifier on
Solana). The trust is entirely in **how the EVM mint learns a Solana burn
happened** — and it does NOT need the Solana *root*: because `burn` already
enforces a valid on-chain root before recording a nullifier, mirroring Solana's
**burned-nullifier set** is sufficient (`OpaqMint.pendingMint`). Everything else
stays ZK-bound: the proof binds the nullifier to `(token, amount, dest_address)`,
so the attestor relays a boolean per burn and never sees a secret. The residual
trust is purely *"the attestor won't fabricate a burn that didn't happen."*

**The trust ladder** (each rung strictly reduces trust; `OpaqMint` is built at
rung 1 and the contract is unchanged across 1→3 — only *who* `operator` is moves):

1. **Single operator.** `operator` is one key that calls `addPending`. Trust: one
   party honestly mirrors finalized Solana burns. Simple, deployable, what we ship.
2. **ICP canister as operator.** Point `operator` at an ICP canister's threshold-
   ECDSA-derived Ethereum address. The canister watches Solana via HTTPS outcalls
   to N public RPC nodes (results agreed by the subnet) and signs+posts `addPending`
   itself (chain-key tECDSA) — a consensus-run, on-chain, auto-signing oracle, no
   relayer server ("near-zero-infra"). Threshold Ed25519 signs Solana txs too, so
   the reverse direction (EVM burn → Solana mint) is symmetric. Trust shifts from
   one party to *the ICP subnet + the honesty of the queried RPC nodes* (the outcall
   proves "the subnet agreed on what the RPCs returned," not that the RPCs were
   truthful about Solana consensus).
3. **ICP canister + in-canister Solana light client.** The canister verifies Solana
   *consensus* (validator-set signatures / bank hash for the block containing the
   burn) instead of trusting RPC, then bridges via its threshold signature. The
   heavy consensus-verification runs as cheap canister compute, so neither L1 runs
   the other's light client on-chain. Trust reduces to *the ICP subnet* (validity
   now comes from verified Solana consensus). This is ICP's "chain fusion" thesis.
4. **Full on-chain light clients both sides.** Trust only the two L1s — the ideal,
   but verifying Solana consensus on the EVM is prohibitively expensive, which is
   exactly the cost rung 3 sidesteps.

So Phase 3 is **not** trustless-by-default, but it has a concrete, monotonic path
from one operator to near-trustless, and the EVM contract is already built to walk
it (`operator` = a canister address is a deployment config, not a rewrite).

### A.10 Technical Notes

**Why Poseidon over keccak/sha256 here:**
Poseidon is designed to be cheap inside arithmetic circuits (few constraints per call) versus keccak/sha256 which are expensive in-circuit despite being cheap natively. Since nearly every operation in this project happens inside a circuit, Poseidon is the right default.

**Why BN254 specifically:**
- Solana: native `alt_bn128_addition`, `alt_bn128_multiplication`, `alt_bn128_pairing` syscalls
- Ethereum: native `ecAdd`, `ecMul`, `ecPairing` precompiles (same curve, since EIP-196/197)
- Noir/Barretenberg: default proving backend operates over BN254
This alignment is why the cross-chain extension (Phase 3) doesn't require a different proof system on each side.

**UltraHonk vs UltraPlonk vs Groth16:**
Barretenberg has moved toward UltraHonk as the modern default; check current `bb` defaults at implementation time, since the verifier math (and therefore the Solana verifier code) differs between the two. Pin a specific `bb`/Noir version early and don't drift mid-project. **However:** Honk's heavier on-chain verification may not fit Solana's per-transaction compute budget — see B.6. Groth16 (what Light Protocol actually runs on Solana) is the live Phase 1 fallback if the Honk verifier doesn't fit; that decision must be made before the public-input layout is frozen.

**Cost implication of single-PDA design:**
A single growing account (vs PDA-per-nullifier) avoids paying a full rent-exempt minimum on every withdraw/transfer — only incremental rent for appended bytes. Combined with BN254 syscall costs (addition: 334 CU, multiplication: 3,840 CU, single pairing: ~36,364 CU), this pushes realistic per-transaction cost on Solana below typical EVM L2 privacy-pool costs — worth benchmarking precisely once Phase 1 is built.

**Recent-root history:**
Since proofs are generated against a merkle root that may move (other deposits/transfers landing first), the tree keeps a small ring buffer of recent valid roots rather than only the latest — standard pattern from Tornado Cash and descendants.

### A.11 Prior Art & Why Opaq Is Different

| Project | Chain | Notes |
|---------|-------|-------|
| Tornado Cash | Ethereum | Single-asset pools (one pool per denomination), no native multi-asset UTXO |
| Railgun | Ethereum/EVM | Multi-asset UTXO privacy, closest conceptual relative, EVM-only |
| Light Protocol | Solana | Existing Solana privacy infra, uses BN254 precompiles — closest Solana prior art, worth studying their verifier code directly |
| Aztec | Own L2 | Full private execution environment, much larger scope than a privacy pool |

Opaq's distinguishing bet: Noir-based circuits (rather than custom halo2/circom)
plus a deliberate BN254-everywhere design specifically to make Solana↔EVM
burn/mint share a proof system instead of needing two incompatible ones.

### A.12 Phase 1 Privacy Limitations (public amounts — read before trusting the pool)

Phase 1 is a working privacy *pipeline*, not yet a strong privacy *guarantee*.
Both `deposit` and `withdraw` expose `amount` and `token_id` in the clear, so
the effective anonymity set for any withdrawal is not "all leaves in the tree" —
it is only the set of deposits sharing the *exact same* `(token_id, amount)`.

Things to be honest about rather than gloss over:

- **Amount fingerprinting.** Depositing 137.42 of a token and later withdrawing
  137.42 of it is near-trivially linkable. Tornado Cash used fixed
  denominations specifically to keep these buckets large; Opaq trades that for
  arbitrary-amount flexibility, which shrinks each bucket — sometimes to one.
- **Timing correlation.** A withdraw shortly after the only matching-amount
  deposit links the two regardless of the cryptography.
- **The cryptography is sound; the metadata is the leak.** None of this is a
  proof-system flaw — it is information deliberately made public on the I/O legs.

What actually closes this is **Phase 2's join-split** (`transfer.nr`), where
value moves between commitments without amounts being revealed and change
outputs break the 1:1 deposit↔withdraw amount match. Until then, treat Phase 1
as "unlinkable only within an identical-amount, identical-token crowd," and have
the CLI warn when a withdrawal amount is rare enough to be self-identifying —
the same education-at-the-tooling-layer stance as the recipient-history warning
in A.8. Don't market Phase 1 as more than it is.

---

## Part B — Implementation Spec

### B.0 Non-Negotiable Pre-Flight Check (do this before writing any pool logic)

Per A.7, resolve the Poseidon variant question first, as a standalone spike.

**Mandatory acceptance test before any circuit/program code is written:**
1. Pick two arbitrary field elements.
2. Hash them with `poseidon::bn254::hash_2` (from the `noir-lang/poseidon` library, NOT `std::hash`) in a throwaway Noir program.
3. Hash the same two elements off-chain with the `light-poseidon` crate (Rust).
4. Hash the same two elements on-chain via `solana_poseidon::hashv` (deploy a trivial test instruction on devnet, log the result).
5. Confirm all three outputs are byte-identical.

Do not proceed to circuit writing until this passes. If it doesn't pass on
the first try, the likely culprits are: byte order (big-endian vs
little-endian — Solana's syscall and `light-poseidon` both default to
big-endian; verify Noir's field-to-bytes serialization matches), or input
domain separation (number of inputs / padding convention).

### B.1 Toolchain — Pinned Versions

Versions below were current as of late June 2026. Re-verify each before
starting, since this stack moves fast — pin exact versions in lockfiles
immediately on install and do not drift mid-project.

| Tool | Pin to | Install |
|------|--------|---------|
| Noir / `nargo` | `1.0.0-beta.22` (installed & verified by M0; newer beta than the original beta.20 pin, sanctioned per this row's guidance) | `noirup --version 1.0.0-beta.22` |
| Barretenberg / `bb` | `5.0.0-nightly.20260522` (installed; verify it pairs with nargo beta.22 before proof work at M1) | installed via `bbup` or bundled with `noirup` |
| `noir-lang/poseidon` library | `v0.3.0` (verified compatible with nargo beta.22 by the M0 spike). **Module path is `poseidon::poseidon::bn254::*`, not `dep::poseidon::bn254::*`** — see B.4.1. | `poseidon = { git = "https://github.com/noir-lang/poseidon", tag = "v0.3.0" }` in `Nargo.toml` |
| `solana-program` (Rust crate) | `3.x` (matches Agave 3.0.15). The program is barebone native Solana: hand-rolled instruction dispatch on a tag byte, `borsh` for account (de)serialization, no framework macros, no IDL. | `solana-program = "3"` in Cargo.toml |
| `borsh` (Rust crate) | `1.x` | `borsh = "1"` in Cargo.toml |
| Solana CLI / Agave | `3.0.15` (installed & used for the M0 validator deploy) | `agave-install` |
| **SBF platform-tools** | **`v1.54` (ships rust/cargo `1.89`).** The default bundled with `cargo build-sbf` here is **v1.51 (cargo 1.84)**, which **cannot build any solana 3.x program** — the 3.x dep graph transitively requires `edition2024` (`wincode`, `zeroize 1.9`, `blake3 1.8 → cmov`, `toml_edit 0.25`…), unsupported before cargo 1.85, and `build-sbf` fails at manifest-parse time. Install once: `cargo build-sbf --install-only --force-tools-install --tools-version v1.54`, then **always pass `--tools-version v1.54`** to `cargo build-sbf`. Discovered while closing M0's on-chain leg. | `cargo build-sbf --tools-version v1.54` |
| Rust | toolchain pinned via `rust-toolchain.toml` at repo root | `rustup` |
| `light-poseidon` (Rust crate, off-chain prover) | `^0.4.0` | Cargo dependency |
| `solana-poseidon` (Rust crate, on-chain program) | latest `3.x` (confirm exact patch on crates.io at setup time) | Cargo dependency |
| `ark-bn254` (Rust crate) | `^0.5.0` (match whatever `solana-poseidon` 3.x pins internally to avoid duplicate-version build errors) | Cargo dependency |

**Action item:** at the start of Step 1, run a version-check script that
prints all installed tool versions, commit it as `scripts/check-versions.sh`,
and re-run it any time something inexplicably breaks — version drift is the
single most likely source of "works on my machine" bugs in this stack.

### B.2 Resolved Design Decisions

These were open questions earlier in the design process; they are now
decided so the agent doesn't need to pause and ask.

**Merkle tree depth: 24.**
Supports up to 2^24 (~16.7M) note commitments — generous headroom. Each
withdraw/transfer proof needs to verify a 24-step Merkle path; at ~586
constraints per Poseidon `hash_2` call (per published Noir benchmarks), that's
~14,000 constraints for the path alone — trivial for Barretenberg. If this
number needs to change later, it's a single constant, not a redesign.

**`owner_pubkey` derivation from `spend_key`:**
```
owner_pubkey = poseidon::bn254::hash_1([spend_key])
```
One Poseidon hash. `spend_key` is the secret the note-holder must know;
`owner_pubkey` is what's embedded in the commitment. Intentionally the
simplest possible derivation — no need for anything more elaborate at this
stage.

**On-chain tree storage layout (store the frontier, not every leaf):**
Storing every leaf on-chain is wasteful and unnecessary. Use the standard
incremental Merkle tree pattern (same approach Tornado Cash's
`MerkleTreeWithHistory.sol` uses, adapted to a native, borsh-serialized PDA
account):

```rust
pub struct CommitmentTree {
    pub next_index: u64,                          // next free leaf slot
    pub filled_subtrees: [[u8; 32]; 24],          // one hash per tree level
    pub roots: [[u8; 32]; 32],                    // ring buffer of recent roots
    pub current_root_index: u8,                   // ring buffer cursor
}
```
Inserting a leaf updates `filled_subtrees` bottom-up and writes a new root
into the ring buffer — O(depth) work per insert, not O(tree size). This
account has a fixed, small size (no `realloc` needed for the tree itself).

**Empty-tree zero values (precompute these):** the incremental tree needs a
precomputed `zeros[i]` table — the hash of an all-empty subtree at each level —
to initialize `filled_subtrees` and to compute `roots[0]` (the genesis root) at
`initialize_pool`. Define `zeros[0]` as a fixed zero-leaf sentinel and
`zeros[i] = poseidon::hash_2([zeros[i-1], zeros[i-1]])`. This table must be
generated with the *same* Poseidon (original, Circom params — A.7) the circuit
and program use; getting the zero convention wrong is the same "roots never
match" failure family as the Poseidon-variant mismatch, so include a check
against the circuit's empty-tree root in the B.0 parity work.

**`NullifierSet` storage layout:**
Append-only flat array of 32-byte nullifiers, unsorted, with linear scan for
membership check on insert (`O(n)`), grown via `realloc` in fixed-size chunks
(e.g. 1,000 entries / 32,000 bytes per growth step, to amortize `realloc`
calls rather than growing by 32 bytes every single time):
```rust
pub struct NullifierSet {
    pub count: u64,
    pub nullifiers: Vec<[u8; 32]>,   // grown via realloc as needed
}
```
**Known scaling limit, explicitly accepted for Phase 1:** linear scan means
withdraw/transfer cost grows with total historical nullifier count. Fine for
an MVP. If/when this becomes a real bottleneck (thousands of nullifiers), the
fix is a sorted array + binary search, or a hash-table layout — a documented
Phase 1.5 optimization, not a Phase 1 blocker.

**Why single PDAs instead of PDA-per-nullifier:**
A fresh PDA per nullifier pays the full rent-exempt minimum (~0.00089 SOL) on
every withdraw/transfer regardless of actual data size. A single growing
account only pays incremental rent for the bytes appended — meaningfully
cheaper at scale, and avoids thousands of tiny derived accounts cluttering
the chain.

### B.3 Repository Bootstrap (Day 0)

```bash
# 1. Toolchain
noirup --version 1.0.0-beta.20
agave-install init <latest-stable>

# 2. Scaffold — plain cargo workspace
cargo new --lib opaq --vcs none
cd opaq
mkdir -p circuits/deposit circuits/withdraw crates/prover crates/common programs/opaq/src
cargo new --lib programs/opaq --vcs none   # solana-program + borsh

# 3. Noir circuit packages
cd circuits/deposit && nargo init --name deposit && cd ../..
cd circuits/withdraw && nargo init --name withdraw && cd ../..

# 4. Rust workspace for prover/common — add to root Cargo.toml [workspace] members

# 5. Local validator for iteration
solana-test-validator
```

Add `rust-toolchain.toml` pinning the Rust version immediately, before
writing any code, so the whole team/agent session builds identically.

### B.4 Circuit Specs (Noir)

**B.4.1 `circuits/deposit/src/main.nr`**

```rust
// Import path note: in noir-lang/poseidon v0.3.0 (nargo 1.0.0-beta.22) the path
// is `poseidon::poseidon::bn254::*` — crate `poseidon`, module `poseidon`,
// submodule `bn254`. No `dep::` prefix (removed in modern Noir). Verified by the
// M0 spike in circuits/poseidon_check.
use poseidon::poseidon::bn254::hash_4;

// Purpose of this proof: it is what BINDS the deposited amount/token (public
// inputs, which the on-chain `deposit` instruction checks against the actual
// SPL transfer) to the commitment inserted into the tree. The contract cannot
// recompute the commitment itself (owner_pubkey and blinding_factor are
// secret), so without this proof a depositor could transfer 100 tokens but
// insert a commitment encoding 1,000,000 and later drain the vault. The
// on-chain program MUST verify this proof and assert its public token_id/amount
// match the instruction arguments and the transferred amount — see B.5.2.
fn main(
    token_id: pub Field,
    amount: pub Field,
    new_commitment: pub Field,
    owner_pubkey: Field,
    blinding_factor: Field,
) {
    // amount must fit in u64: on-chain amounts are u64 and field arithmetic
    // wraps mod p. An unconstrained amount would (critically, once the Phase 2
    // join-split enforces value conservation) enable underflow/overflow attacks
    // that mint value. Constrain it from day one so the habit is in place.
    amount.assert_max_bit_size::<64>();

    let computed = hash_4([token_id, amount, owner_pubkey, blinding_factor]);
    assert(computed == new_commitment);
}
```

**B.4.2 `circuits/withdraw/src/main.nr`**

```rust
use poseidon::poseidon::bn254::{hash_1, hash_2, hash_4};  // path note: see B.4.1

global TREE_DEPTH: u32 = 24;

fn main(
    merkle_root: pub Field,
    nullifier: pub Field,
    token_id: pub Field,
    amount: pub Field,
    recipient: pub Field,           // Solana pubkey, encoded as Field
    spend_key: Field,
    blinding_factor: Field,
    merkle_path: [Field; TREE_DEPTH],
    merkle_path_indices: [bool; TREE_DEPTH],  // false = current is left, true = current is right
                                              // (was `u1`; u1 removed in nargo beta.22 — use bool)
) {
    amount.assert_max_bit_size::<64>();   // see B.4.1 — required for value safety

    let owner_pubkey = hash_1([spend_key]);
    let commitment = hash_4([token_id, amount, owner_pubkey, blinding_factor]);

    // Merkle membership check
    let mut current = commitment;
    for i in 0..TREE_DEPTH {
        let sibling = merkle_path[i];
        current = if merkle_path_indices[i] {
            hash_2([sibling, current])
        } else {
            hash_2([current, sibling])
        };
    }
    assert(current == merkle_root);

    // Nullifier check
    let computed_nullifier = hash_2([commitment, spend_key]);
    assert(computed_nullifier == nullifier);

    // recipient is a public input — bound into the proof so it can't be swapped
    // after generation, but otherwise unconstrained. `let _ = recipient` lets
    // the optimizer drop it from the public inputs; use `std::as_witness(recipient)`
    // to KEEP it as a verified public input (confirmed via the compiled ABI).
    std::as_witness(recipient);
}
```

Note: `recipient` doesn't need an algebraic constraint inside the circuit —
its role is being a public input that's part of what gets verified,
preventing a relayer/submitter from redirecting funds to a different address
than the prover intended.

**Field-encoding of 32-byte values (`token_id`, `recipient`) — mandatory.**
A Solana `Pubkey` (or SPL mint) is 32 bytes = 256 bits, which does **not** fit
in a BN254 field element (~254 bits). Casting one raw into a `Field` silently
wraps mod p and lets two distinct pubkeys collide — for `recipient` that is a
fund-redirection bug, for `token_id` a cross-token-accounting bug. So both
public inputs are the *canonical* encoding:

```
to_field(bytes32) = poseidon::bn254::hash_2([
    field(bytes32[0..16]),   // high 128 bits, big-endian
    field(bytes32[16..32]),  // low  128 bits, big-endian
])
```

- `token_id` public input  = `to_field(mint)`
- `recipient` public input = `to_field(recipient_pubkey)`

This must be computed **identically** in three places — the prover
(`light-poseidon`), the circuit (where `token_id` enters the commitment hash),
and the on-chain program (`solana-poseidon`, which reconstructs both values
from the instruction's `Pubkey` arguments and checks them against the proof's
public inputs). Poseidon collision-resistance is what makes the binding sound:
an attacker can't find a different `recipient`/`mint` hashing to the same field.
This is the same byte-order/domain-separation surface the B.0 parity spike
covers — extend that test to cover `to_field` on 32-byte inputs too.

**B.4.3 `circuits/transfer/src/main.nr` — Phase 2 (now written, P2.0)**

Built after Phase 1 round-tripped on devnet. Fixed **2-in/2-out** join-split,
fully private: `token_id` and every amount are private witnesses; the only public
inputs are `merkle_root`, `nullifier[2]`, `out_commitment[2]`. So the anonymity
set is every transfer, not identical-(token,amount) ones (closes A.12). Design:
- All 4 notes reuse one private `token_id` (a transfer can't mint across tokens).
- Value conservation `Σin == Σout`, every amount range-checked to 64 bits — the
  range checks are load-bearing for soundness (field wraps mod p; an unchecked
  amount near p forges value, B.4.1). Sums stay `< 2·2^64 < p`, so the equality
  is exact.
- `is_dummy` inputs (amount 0, skip Merkle membership) let a transfer spend < 2
  real notes; their nullifier is still bound to a fresh-blinded commitment, so it
  can't collide with or pre-empt a real note's nullifier (Poseidon preimage
  resistance) and recording it on-chain is harmless.

Compiles (33.6k ACIR opcodes, 0 Brillig — only `AssertZero` + `RANGE`, the
soundly-lowered set per B.6). Remaining: host-parity witness gen + Groth16
prove/verify (needs ceremony power ~17–18, ~2× withdraw), then the on-chain
`transfer` instruction (tag 3: verify → root-recent → 2 nullifiers → 2 inserts,
no vault), `vk_transfer`, an `opaq transfer` CLI, and a deposit→transfer→withdraw
e2e. NOT yet proven.

### B.5 Solana Program Spec (native Solana)

The program is `solana-program` + `borsh` only. `process_instruction` dispatches
on the first byte of instruction data (0/1/2/3/4 = `initialize_pool`/`deposit`/
`withdraw`/`transfer`/`burn`); every account struct below is a plain
`#[derive(BorshSerialize, BorshDeserialize)]` type manually (de)serialized from
PDA account data — no framework macros, no generated IDL.

**B.5.1 Accounts**

No separate `Vault` state account: the vault is the SPL token account itself —
the canonical Associated Token Account of a `vault_authority` PDA
(`[VAULT_SEED]`) for the given mint, re-derived and checked on every
deposit/withdraw (B.2 "why single PDAs"). `CommitmentTree` and `NullifierSet`
are the only custom accounts, both single global PDAs:

```rust
#[derive(BorshSerialize, BorshDeserialize)]
pub struct CommitmentTree {
    pub next_index: u64,
    pub filled_subtrees: [[u8; 32]; 24],
    pub roots: [[u8; 32]; 32],
    pub current_root_index: u8,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct NullifierSet {
    pub count: u64,
    pub nullifiers: Vec<[u8; 32]>,
}
```

**B.5.2 Instructions**

`initialize_pool()`
- One-time setup: creates `CommitmentTree` (zeroed frontier, root[0] = empty-tree root) and `NullifierSet` (empty) PDAs.

`deposit(proof: Vec<u8>, token_id: Pubkey, amount: u64, commitment: [u8; 32])`
- **Verify the deposit proof** (B.4.1) via the BN254 verifier (B.6), with public inputs `(token_id_field, amount, commitment)`.
- **Bind the proof's public inputs to the instruction — the single most security-critical check in the deposit path.** Assert: the proof's public `amount` equals the `amount` argument; the proof's public `token_id_field` equals `to_field(token_id)` (B.4.2); the proof's public `commitment` equals the `commitment` argument. Without this binding a depositor can transfer N tokens but commit to a value `> N` and later drain the vault.
- SPL `transfer` **exactly `amount`** of `token_id` from depositor's token account to `Vault` PDA (create `Vault` for this mint if it doesn't exist yet — first deposit of a new token initializes its vault). The transferred amount must equal the proof-bound `amount`.
- Insert `commitment` into `CommitmentTree` (update frontier, push new root into ring buffer)
- Emit event `Deposited { token_id, amount, commitment, leaf_index }`

`withdraw(proof: Vec<u8>, merkle_root: [u8; 32], nullifier: [u8; 32], token_id: Pubkey, amount: u64, recipient: Pubkey)`
- Derive the field-encoded public inputs from the instruction's `Pubkey` arguments: `token_id_field = to_field(token_id)` and `recipient_field = to_field(recipient)` (B.4.2).
- Verify the Noir/Barretenberg proof against public inputs `(merkle_root, nullifier, token_id_field, amount, recipient_field)` using `alt_bn128` syscalls (the BN254 verifier — see B.6). Because `recipient_field` is a bound public input, a submitter/relayer cannot redirect funds to a different address.
- Check `merkle_root` exists in `CommitmentTree.roots` (the ring buffer, not just the latest)
- Check `nullifier` is not already in `NullifierSet` (linear scan per B.2) — if absent, append it; if present, **reject the transaction**
- SPL `transfer` `amount` of `token_id` from `Vault` to `recipient`
- Emit event `Withdrawn { token_id, amount, nullifier, recipient }`

**Phase 1 explicitly excludes:** `transfer` instruction, any cross-chain instructions, multi-input/output logic.

### B.6 BN254 Verifier — the genuinely hard part

`bb` generates a Solidity verifier turnkey; it does **not** generate a Solana
verifier. This must be hand-built. Concretely:

1. Run `bb write_vk` against the compiled withdraw circuit to get the verification key.
2. Study the verification key structure and the pairing-check equation Barretenberg's UltraHonk verifier uses (the published math, not Barretenberg's Solidity codegen, since Solana needs different serialization/calling conventions than EVM).
3. Implement the equivalent checks in Rust using `solana_program::alt_bn128::{alt_bn128_addition, alt_bn128_multiplication, alt_bn128_pairing}` syscalls.
4. **Before wiring this into the pool program**, write a standalone test: generate a valid proof with `bb prove`, generate an invalid one (flip one byte), and confirm the Solana-side verifier accepts the valid one and rejects the invalid one. Do this in isolation, outside the pool program, before integrating.

**Reference material to study first, not to copy blindly (license/correctness
varies):** Light Protocol's on-chain BN254 verifier code (they already do
Groth16-style verification on Solana with these exact syscalls) — closest
real-world prior art for this exact problem.

**Compute-budget feasibility — settle this BEFORE polishing circuits.**
UltraHonk verification is materially heavier on-chain than Groth16: larger
proofs and many more scalar-multiplication / MSM operations, on top of the
pairing. The per-syscall costs in A.10 are fine in isolation, but a full Honk
verifier may need dozens of `alt_bn128_multiplication` calls and can approach or
exceed Solana's per-transaction compute ceiling — potentially forcing CU-limit
requests, or splitting verification across multiple transactions (which reopens
atomicity questions for the nullifier/tree update). Critically, the closest
real-world prior art cited below (Light Protocol) verifies **Groth16**, not
Honk, precisely because Groth16's constant ~3-pairing verifier fits Solana
comfortably.

**Therefore, as an explicit early gate (see M3 in B.9):** estimate or measure
the worst-case CU of the Honk verifier for the `withdraw` circuit *before*
investing in circuit polish. If it does not fit a single transaction's budget
with comfortable margin, fall back to **Groth16** for Phase 1 — accept the
per-circuit trusted setup as the cost of a Solana-proven, cheap-to-verify path,
and revisit Honk later. This choice changes the verifier's public-input
serialization, so it must be made before the public-input layout is frozen, not
after. Honk's "no trusted setup" advantage is real but is not worth a Phase 1
that can't verify a proof in one transaction.

**M0.5 RESULT (measured 2026-06-26, this validator — see scripts/m05-cu.sh):**
Per-op CU measured on-chain via `programs/cu-probe`:
`alt_bn128` G1 mul = **3,873 CU**, G1 add = **367 CU**, BN254 Fr mul in pure BPF
(ark-ff) = **2,036 CU** (Solana has no field-mul syscall — sumcheck field work
gets no acceleration). Plugged into verifier-cost models for the withdraw
circuit (~2^15 gates, d=15, ~44 entities, 5 public inputs):

| backend | EC ops | field ops | total | vs 1.4M ceiling |
|---------|--------|-----------|-------|-----------------|
| **UltraHonk** | ~365k CU (fits) | 2,000–8,000 Fr muls | **4.4M–16.7M CU** | **3×–12× OVER** |
| **Groth16** | ~170k CU | ~none | **~272k CU** | **FITS (>5× headroom)** |

The Honk EC portion fits comfortably; the sumcheck **field arithmetic** is what
blows the budget (a probe of just 2,000 BPF Fr-muls exhausted the entire 1.4M
budget mid-run). Groth16 has no sumcheck and matches Light Protocol's published
~200–300k CU. **Decision: Phase 1 uses Groth16.** Honk-on-Solana would require
splitting verification across multiple transactions with a verification-state
account — breaking nullifier/tree-update atomicity — and is out of scope.

**Open follow-on (blocks M1+ backend work):** `bb` produces UltraHonk/UltraPlonk,
**not Groth16** — Barretenberg has no Groth16 backend. Noir→Groth16 is not
turnkey. The closest Solana prior art (Light Protocol) writes circuits in
**Circom** and proves Groth16 via snarkjs/arkworks. So adopting Groth16 forces a
circuit-toolchain decision that touches A.1's "Circuit language: Noir" choice —
resolve before writing the verifier (B.6) or re-doing circuits. Options: (a)
Circom + Groth16 (Light's path, proven on Solana, loses Noir readability); (b)
a Groth16 backend for Noir's ACIR (research-grade, risky); (c) stay Noir+Honk
with split verification (large, breaks atomicity). The M1/M2 Noir circuits
already built still encode the correct constraints and port directly to Circom.

**RESOLVED (2026-06-26): option (b), Noir ACIR -> Groth16, working end-to-end.**
We forward-ported the only existing tool ([jamesbachini/Noir-Groth16], pinned to
noir beta.19) to our beta.22 toolchain (the ACIR API moved: `current_witness_index`
removed, `MemOp`/blackbox opcodes restructured). Pipeline: `nargo compile` ->
`noir-cli interop` (.r1cs/.wtns) -> snarkjs Groth16 (BN254) setup/prove/verify.
**Both `deposit` and `withdraw` prove and verify**, with snarkjs public signals
matching the circuits' public inputs exactly. Vendored as a patch + setup script
in `tools/noir-groth16/`; run via `scripts/groth16-prove.sh <circuit>`.
Caveat: unaudited code requiring re-port on each Noir upgrade — a minimal owned
ACIR->R1CS (only `AssertZero` + `RANGE` are needed) stays the cleaner long-term
option.

**M3 (in progress): Groth16 verification via `groth16-solana` — off-chain leg
passing.** `crates/groth16-verify` converts snarkjs (BN254) proofs/vk to
`groth16-solana`'s byte layout and verifies with the same crate used on-chain:
the deposit proof **verifies** and a tampered proof is **rejected**. Conversion
conventions (now settled, verified against `solana-bn254`'s PodG1/PodG2):
big-endian; G1 = x‖y with the point at infinity as all-zeros and Z-normalization;
G2 in **EIP-197 imaginary-first** order (x_c1‖x_c0‖y_c1‖y_c0); **proof_a's y
negated**. **On-chain leg also passing** (`programs/groth16-verify-check` +
`scripts/m3-onchain.sh`): the deposit VK is embedded as Rust consts and the same
`groth16-solana` verify runs in the SBF VM on `solana-test-validator` —
**accepts the valid proof, rejects a tampered one** via real `sol_alt_bn128`
syscalls. **M3 done** (modulo the insecure ceremony below). The hand-built
UltraHonk verifier B.6 originally feared is moot — Groth16 verification is the
audited `groth16-solana` crate, no pairing math hand-rolled.

> **SECURITY CAVEAT — insecure proving ceremony.** `scripts/groth16-prove.sh`
> (via the upstream `run_circuit.sh`) generates powers-of-tau with **no
> contribution** — trivial toxic waste, so the resulting vk is degenerate
> (`vk_alpha_1` = generator, some `IC` points at infinity) and **anyone can
> forge proofs**. Fine for wiring/testing the verifier, but a **real
> Phase 1.5/production ceremony is mandatory before deployment**. The infinity
> `IC` points are an artifact of the trivial setup, not a circuit bug — a proper
> ceremony yields non-degenerate `IC` that bind every public input.
>
> **Ceremony tooling now exists** (`ceremony/`, `scripts/ceremony-*.sh`): it
> reuses the Perpetual Powers of Tau (Hermez power-16) for the universal phase-1
> and runs a per-circuit phase-2 (multi-contribution + drand beacon + verify) for
> `deposit`/`withdraw`, re-embedding the VKs via `emit_artifacts --real`. The
> `--smoke` profile is verified end-to-end (produces verifying proofs), but a
> trustworthy run still requires **independent contributors + a pinned beacon** —
> the social step the scripts can't supply. See `ceremony/README.md`. The embedded
> VKs remain the insecure test ones until that real run happens.

[jamesbachini/Noir-Groth16]: https://github.com/jamesbachini/Noir-Groth16

**Time-boxing advice:** this step alone could take longer than everything
else combined if the verifier math has any subtlety the agent misjudges.
Budget accordingly — if B.0's Poseidon check passes but this step stalls, the
Poseidon work isn't wasted, but the project timeline should flex around this
being the genuine bottleneck.

### B.7 Off-Chain Prover CLI Spec

```bash
opaq deposit --token <mint> --amount <u64> --owner-key <path-to-keyfile>
opaq withdraw --note <note-file> --recipient <pubkey>
```

**`deposit` flow:**
1. Generate fresh `blinding_factor` (random Field element)
2. Compute `owner_pubkey` and `commitment` locally (matching the circuit's hash calls — use `light-poseidon`)
3. Build Noir witness, run `bb prove` for `deposit.nr`
4. Submit `deposit` instruction with proof + public inputs (token_id, amount, commitment)
5. On success, write a note file: `{ token_id, amount, owner_pubkey, blinding_factor, spend_key, leaf_index }`, encrypted at rest with a passphrase (use a standard authenticated encryption scheme — e.g. `age` or `chacha20poly1305` — do not roll custom crypto here)

**`withdraw` flow:**
1. Load note file, decrypt with passphrase
2. Fetch current `CommitmentTree` account via `getAccountInfo`, parse `filled_subtrees`/`roots` locally
3. Reconstruct the Merkle path for this note's `leaf_index` (requires tracking sibling values — either by replaying all `Deposited` events from genesis via `getSignaturesForAddress` + log parsing, or by maintaining a local cache updated incrementally; for Phase 1, full replay from genesis is acceptable since the pool will have few deposits)
4. Compute `nullifier` locally
5. Build Noir witness, run `bb prove` for `withdraw.nr`. The witness's `token_id` and `recipient` public inputs must be the canonical `to_field(...)` encodings (B.4.2), computed with `light-poseidon` so they match what the on-chain program reconstructs from the `Pubkey` arguments — a raw byte cast here will silently fail proof verification.
6. Submit `withdraw` instruction with proof + public inputs
7. Warn the user if the withdrawal `amount` is rare (self-identifying) or the chosen `recipient` has prior on-chain history (per A.8 / A.12), before broadcasting

### B.8 End-to-End Test Scenarios

These are the acceptance criteria for "Phase 1 is done." Each should be a
real, scripted, repeatable test against `solana-test-validator` (or devnet),
not a manual one-off.

**Test 0 — Poseidon parity (run first, blocks everything else):**
Per B.0. Automated script comparing Noir/`light-poseidon`/on-chain-syscall outputs for several input combinations (not just one).

**Test 1 — Happy path deposit/withdraw:**
1. Airdrop SOL + mint a test SPL token to a test wallet
2. `opaq deposit --token <test-mint> --amount 100`
3. Assert: vault balance increased by 100, a `Deposited` event was emitted, a note file was written
4. `opaq withdraw --note <file> --recipient <fresh-pubkey>`
5. Assert: recipient's token balance is 100, vault balance decreased by 100, a `Withdrawn` event was emitted

**Test 2 — Double-spend rejection:**
1. Repeat Test 1 through step 4
2. Attempt `opaq withdraw` again with the **same note file**
3. Assert: transaction fails, nullifier-already-exists error, vault balance unchanged

**Test 3 — Wrong amount/forged public input rejection (withdraw):**
1. Deposit 100 tokens
2. Manually construct a withdraw transaction claiming `amount = 1000` (proof was generated for 100)
3. Assert: proof verification fails on-chain, transaction rejected
4. Variant: keep `amount = 100` in the proof but pass a different `recipient` Pubkey in the instruction than the one the proof was generated for. Assert: `recipient_field` binding check fails, transaction rejected (funds can't be redirected).

**Test 3b — Deposit binding / over-commit rejection (the pool-drain guard):**
1. Construct a `deposit` that SPL-transfers 100 tokens but supplies a `commitment` (and matching proof) encoding `amount = 1_000_000`.
2. Assert: the deposit instruction rejects it because the proof's public `amount` (1,000,000) doesn't match the transferred/instruction `amount` (100). This is the direct test of B.5.2's critical binding — if it ever passes, the vault is drainable.
3. Variant: supply a well-formed proof for amount = 100 but an instruction `amount` argument of 100 while SPL-transferring only 1. Assert: rejected (transferred amount must equal the bound amount).

**Test 4 — Stale root tolerance:**
1. Deposit note A
2. Deposit note B (this changes the tree root)
3. Withdraw note A using the **root from before note B was deposited**
4. Assert: succeeds, because the root ring buffer retains recent roots — validates the ring-buffer design, not just the happy path

**Test 5 — Root ring buffer overflow (edge case):**
1. Deposit more notes than the ring buffer size (32, per B.2)
2. Attempt to withdraw a note using a root that has since been evicted from the ring buffer
3. Assert: fails with a clear "stale root" error, not a confusing generic failure — confirms the failure mode is legible, since this will happen in real usage

**Test 6 — Multi-token isolation:**
1. Deposit token A and token B into the pool
2. Withdraw token A
3. Assert: token B's vault balance is untouched, no cross-token leakage in accounting

**Test 7 — Zero-infra read path:**
1. From a clean machine with only a public RPC endpoint configured (no custom indexer, no special access) reconstruct a Merkle path for an existing note purely from on-chain account data and transaction logs
2. Assert: this succeeds without any non-RPC dependency, validating the zero-infra design goal end-to-end, not just in theory

### B.9 Milestone Checklist (suggested order, not strict gates)

```
[x] M0  — Poseidon parity test passes (B.0), incl. to_field(32-byte) and empty-tree zeros parity
[x] M0.5— Proof-system CU feasibility settled (B.6): Honk verifier fits one tx's compute budget, OR Groth16 chosen as Phase 1 fallback — decided BEFORE freezing public-input layout
[x] M1  — deposit.nr compiles, proves, verifies locally (bb verify, no Solana yet)
[x] M2  — withdraw.nr compiles, proves, verifies locally
[x] M3  — BN254 verifier standalone test passes (B.6) — accepts valid, rejects invalid proof
[x] M4  — CommitmentTree account: insert logic unit-tested in isolation (no SPL, no proof, just tree math)
[x] M5  — NullifierSet account: insert/check logic unit-tested in isolation
[x] M6  — deposit instruction wired end-to-end on local validator (Test 1, steps 1-3)
[x] M7  — withdraw instruction wired end-to-end on local validator (Test 1, steps 4-5)
[x] M8  — Tests 1-6 + recipient-binding PASS on validator: round-trip,
          double-spend (T2), forged-amount deposit & withdraw (T3), stale-root
          tolerance (T4), multi-token vault isolation (T6), wrong-recipient.
          Unblocked by splitting Groth16 setup (fixed zkey/VK, once) from proving
          (per note) — scripts/groth16-setup.sh + groth16-prove-note.sh — so many
          notes verify against one embedded VK. That setup/prove split is also
          the structural prerequisite for a real (secure) ceremony (B.6 blocker).
          Test 5 (ring-buffer overflow) now also PASS: scripts/test5-ringbuffer.sh
          deposits note A then 32 fillers (33 total, wrapping ROOT_HISTORY=32) so
          A's root is evicted, then asserts its still-valid withdraw proof is
          rejected with a clear E_UNKNOWN_ROOT (0x4) — not a generic failure —
          with funds untouched (eviction is also RPC-verified via read_path).
          All of B.8's Tests 1-6 + bindings now pass on a validator.
[~] M9  — `opaq` prover/note CLI (crates/prover): deposit generates secrets +
          derives the commitment + writes an ENCRYPTED note (Argon2id +
          ChaCha20-Poly1305) + emits the circuit inputs.json for the real note;
          withdraw decrypts + derives the nullifier. Both surface the A.12
          amount-fingerprinting and A.8 recipient-history warnings; clean errors
          (e.g. wrong-passphrase). Remaining polish: auto-prove + auto-submit
          (currently prints public inputs / hands off to the pipeline), RPC
          merkle-path reconstruction for withdraw (overlaps M10), and an actual
          RPC recipient-history lookup (the warning is currently advisory).
[x] M10 — Test 7 (zero-infra read path) PASS on a validator. A fresh RPC-only
          client (tests/read_path.mjs) reconstructs a note's Merkle path with no
          indexer: getAccountInfo parses the CommitmentTree frontier + root ring
          buffer; getSignaturesForAddress + getTransaction harvest the ordered
          commitment list (leaf from deposit instruction data, leaf_index from
          the `deposit ok` log). opaq_common::tree::reconstruct_path folds that
          leaf list into an authentication path (unit-tested: it reproduces the
          live incremental-tree root for every index). `opaq withdraw --leaves`
          locates the note's commitment, rebuilds the path, and emits a complete
          withdraw witness — closing M9's withdraw-path gap. scripts/m10-zero-
          infra.sh drives it end-to-end (opaq deposit -> real proof -> on-chain
          -> RPC read -> reconstruct), and then PROVES the reconstructed witness
          against a fixed withdraw zkey and SUBMITS a real withdraw — funds move
          to the recipient using only the RPC-rebuilt path; note B stays untouched.
[x] M11 — Deployed and demoed on Solana devnet (not just local validator).
          scripts/m11-devnet.sh builds with fixed test-ceremony VKs (B.6), deploys
          to devnet (default RPC https://api.devnet.solana.com), and runs Test 1
          (deposit -> withdraw round-trip) via tests/m11_devnet_demo.mjs against
          the public RPC — with RPC 429 retries/pacing for devnet rate limits.
          Deploys a FRESH program keypair each run (clean tree, so the demo note
          is at leaf_index 0 and its index-0 proof's root is current) — reusing a
          program would need the M10 read-path reconstruction. The program id
          therefore changes per run; the source of truth is deploy/devnet-latest.json
          (not a fixed id here). Set OPAQ_DEVNET_RPC for a dedicated endpoint.
```

### B.10 Explicitly Out of Scope for Phase 1

Per A.6's phasing — do not let scope creep in:
- `transfer.nr` / join-split (Phase 2)
- Cross-chain burn/mint, any EVM code (Phase 3)
- Lending/collateral features (Phase 4)
- Relayer infrastructure for gas-funding fresh withdrawal addresses (explicitly accepted as a manual/CLI-warned user responsibility per A.8)
- Sorted/hashtable nullifier set optimization (documented Phase 1.5 item, not a blocker)

If the implementing agent finds itself building any of the above mid-Phase-1, that's a signal to stop and re-check this spec rather than improvise.

### B.11 Next Steps (consolidated roadmap)

**Status: the full protocol is built and end-to-end-verified across all three
phases** — Phase 1 (deposit/withdraw, hardened by 2 audit fixes + CU-benchmarked),
Phase 2 (private transfer with hidden amount *and* token; `opaq
deposit→transfer→withdraw` CLI loop, m12/m13), and Phase 3 (Solana `burn` + EVM
`OpaqMint` cross-chain mint, m14/m15). The ceremony tooling (#1) and a focused
audit self-review (#2) are done. It is nonetheless a **verified research
implementation, not a deployable pool**: every embedded VK is still the insecure
test VK, the bridge's relay/operator isn't built, and transfers need out-of-band
note delivery. The numbered items (1)–(6) below are the per-area status records;
the prioritized pipeline of what to do *next* is right here:

**The pipeline from here (in priority order):**

1. **[BLOCKER] Run the real ceremony** for all four circuits (deposit, withdraw,
   transfer, burn). Tooling is ready (`scripts/ceremony-*.sh`); this is the
   operational step — independent contributors + a pinned drand beacon — then
   re-embed VKs + commit the transcript. Gates everything; also *required for
   correctness* on the EVM side (the trivial VK's degenerate points are rejected
   by the EVM `ecMul` precompile — see #6).
2. **[BLOCKER] External audit** of `programs/opaq`, the vendored
   `tools/noir-groth16` backend (ideally replaced by a minimal owned ACIR→R1CS for
   just `AssertZero`+`RANGE`), and `evm/OpaqMint.sol` + the generated verifier. The
   self-review (#2) found+fixed 2 real bugs but is not a substitute.
3. **Make the bridge drivable — FORWARD SIDE DONE (P3.2–P3.5).** `opaq burn`
   (`--prove`/`--submit`, self-served Solana burn; verified by m16) + `evm/mint.mjs`
   (turns the burn proof into an `OpaqMint.mint` call via Foundry `cast`, self-served
   EVM mint; verified by m17) let one person drive burn→mint with **no relayer**, on
   a live validator and anvil. Remaining: the **attestation** gating `addPending` —
   an ICP operator canister (A.9 rung 2) or, zero-infra, an on-chain Solana light
   client (rung 4); and the **reverse direction** (EVM burn → Solana mint).
4. **Received-note discovery** (wallet UX) — **now spec'd, see B.13.** Today
   transfer outputs are handed over out-of-band. B.13 specs an independent
   X25519 `view_key` (separate from `spend_key`, rotatable, per-note encrypted
   memos riding along in the `transfer` instruction's own data — zero circuit
   or ceremony impact) plus `opaq list-unspent`. Not yet implemented (P2.5.0
   in progress).
5. **Phase 1.5 perf** (non-blocking): swap the O(n) nullifier scan for a
   sorted/hash-table set before mainnet scale (measured headroom ~41k nullifiers,
   ~31 CU each).
6. **Deploy/ops**: mainnet program deploy + monitoring, ICP canister hosting, the
   A.9 rung-3 in-canister Solana light client to drop the operator's RPC trust.

The per-area records below (kept for detail; statuses are current):

**1. Secure proving ceremony — HARD BLOCKER (tooling done; live run pending).**
Today the zkeys come from a trivial powers-of-tau with no contributions (B.6
SECURITY CAVEAT), so the verifying key is **forgeable** — anyone could mint a
valid proof and drain the pool. The ceremony **pipeline now exists** (`ceremony/`,
`scripts/ceremony-*.sh`): phase-1 is *reused* from the Perpetual Powers of Tau
(Hermez power-16 — universal, so no need to generate our own), and phase-2 is run
per-circuit (multi-contribution + drand beacon + `zkey verify`) for `deposit` and
`withdraw`, re-embedding VKs via `emit_artifacts --real`. The `--smoke` profile is
verified end-to-end. What remains is **operational, not code**: a real run with
≥1 genuinely independent contributor per circuit (toxic waste destroyed) and a
drand round pinned in advance, then commit the resulting VKs + `transcript.md`.
Until that run, the embedded VKs stay the insecure test ones.

**2. Audit — HARD BLOCKER.** Two surfaces: (a) the native program
(`programs/opaq`) — proof-input binding, PDA/vault checks, nullifier set,
realloc/rent, arithmetic; and (b) the **unaudited** vendored Noir→Groth16
backend (`tools/noir-groth16`, B.6) which lowers ACIR→R1CS and is the thing that
makes the proofs *mean* what the circuits say. A constraint-soundness bug there
is as dangerous as a program bug. Owning a minimal, auditable ACIR→R1CS for just
`AssertZero` + `RANGE` (the only opcodes our circuits emit) is the cleaner
long-term alternative to maintaining/auditing the general-purpose fork.

> A first focused pass on (a) already found + fixed one **critical** bug: the
> SPL `token_program` was taken from the caller and invoked unchecked, so a
> forged/no-op token program let a deposit insert a commitment WITHOUT funding
> the vault (drain on withdraw). Now both instructions pin `SPL_TOKEN_PROGRAM_ID`
> (+ mint-binding defense-in-depth), with an m8 negative test
> ("forged token_program deposit rejected"). A follow-up pass also pins the vault
> to its canonical ATA (low-severity: a non-canonical token account owned by the
> vault PDA could desync deposits/withdrawals from the single-vault invariant — no
> theft, but bad accounting), with an m8 negative test ("non-canonical vault
> deposit rejected").
>
> Surface (b), the vendored Noir→Groth16 lowering, also got a focused soundness
> pass over the opcodes our circuits emit (`AssertZero`, `RANGE`, with Brillig
> hints) — **no under-constraint found**: `AssertZero` lowers each `mul_term` to a
> real `lhs·rhs=tmp` constraint then binds `1·(Σ terms)=0`; `RANGE` boolean-checks
> every bit (`w·(w−1)=0`) AND binds `Σ 2ⁱ·bitᵢ = input` (so it genuinely proves
> `input < 2^n`; ≥field-width ranges are tautological with no overflow since
> `n ≤ 253 ⇒ 2ⁿ < r`); wire 0 is reserved as the ONE signal (witness `w`→wire
> `w+1`), so those constraints aren't vacuous; opcode dispatch is exhaustive
> (nothing silently dropped) and the dangerous curve/MSM/dynamic-memory opcodes we
> don't use are stubbed fail-closed. Empirically corroborated by m8's forged-amount
> deposits/withdraws being rejected on-chain. This was a focused self-review of the
> soundness-critical paths, **not** a full external audit of the whole crate.

**3. Finish M9 (prover CLI polish) — DONE.** The `opaq` CLI now drives the full
lifecycle itself, both directions, verified end-to-end by m10 on a validator:
- `opaq deposit --token <mint> --amount <n> --note <f> --rpc <url> --program <id>
  --submit --payer <kp> --zkey <deposit.zkey>` generates a fresh note, proves the
  binding proof, and signs + broadcasts the deposit (SPL → vault + commitment).
- `opaq withdraw --note <f> --recipient <pk> --rpc <url> --program <id> --submit
  --payer <kp> --zkey <withdraw.zkey>` harvests the leaf set live, reconstructs
  the Merkle path against any pool state (b — closes the M11 fresh-pool shortcut),
  auto-checks the recipient's on-chain history (c — A.8 fresh/not-fresh finding),
  proves, and signs + broadcasts the withdraw.

`--prove` (without `--submit`) stops at the ready-to-submit blob. Chain I/O lives
in the tested node helpers (tests/{read_leaves,recipient_history,submit_*}.mjs)
the CLI orchestrates; the on-chain instruction layout is a single shared lib fn
(`groth16_verify::opaq_instruction`).

**4. Phase 1.5 optimizations (non-blocking).** CU **measured** (A.10, via m10 on a
validator): deposit ~127.6k CU, withdraw ~124.0k CU, and the O(n) nullifier
linear-scan marginal is only **~31 CU/nullifier**. With ~1.276M CU of headroom
under the 1.4M ceiling, a withdraw stays in budget until **~41k nullifiers** — so
the sorted/hash-table nullifier set (B.2, replacing the scan) is confirmed
*non-blocking*: needed before mainnet scale, not before correctness. (Account
size is the other ceiling: 10 MB ÷ 32 B ≈ 327k nullifiers.)

**5. Phase 2 — private transfer + hidden amounts (A.6). DONE (P2.0–P2.4).** A
fully-private 2-in/2-out join-split, end-to-end: the circuit
`circuits/transfer/src/main.nr` proves/verifies (Groth16 at ptau power 17); the
on-chain `transfer` instruction (tag 3: verify → root-recent → record 2
nullifiers → insert 2 commitments, no vault) is live with `vk_transfer`; and the
`opaq transfer` CLI drives it (harvest + reconstruct the input path, mint the
output notes, prove, submit). Verified on a validator by **m12** (instruction +
replay rejection) and **m13** (full CLI loop: `opaq deposit` → `opaq transfer` →
`opaq withdraw` the change). The read path (`tests/read_path.mjs`) now harvests
transfer output commitments too, so transfer-created notes are withdrawable. This
**closes A.12**: amounts and token are hidden in a transfer, so the anonymity set
is every transfer, not identical-`(token, amount)` ones. (Deposit/withdraw still
expose amount/token at the SPL boundary — privacy comes from depositing, then
transferring before withdrawing to a fresh note.) Original framing kept below:
`transfer.nr` N-input/M-output join-split with per-`token_id` value
conservation, which also closes the A.12 limitation (amounts/token are public in
Phase 1, so the anonymity set is only identical-`(token, amount)` transfers).
Until Phase 2, treat the privacy as "unlinkable only within an identical-amount
crowd."

**6. Phase 3 — cross-chain burn/mint (A.6). FORWARD BRIDGE DONE + DRIVABLE (P3.0–P3.5).**
`circuits/burn/src/main.nr` (`withdraw` with `recipient` swapped for the bound EVM
destination `(dest_chain, dest_address)` and no SPL release; 16.4k ACIR, 0 Brillig,
power 16) AND the on-chain `burn` instruction (tag 4: verify → root-recent → record
nullifier, NO tree insert, NO vault release) with `vk_burn` are live — verified on a
validator by **m14**: deposit→burn records the nullifier, leaves the tree and vault
unchanged (value locked on Solana for the EVM mint), and rejects a replay.
**EVM side DONE too** (`evm/`): a Groth16 Solidity verifier (snarkjs
`zkey export solidityverifier` — NOT `bb`, which emits UltraHonk; we're Groth16)
+ `OpaqMint.sol`, verified on anvil by **m15** (`forge test`): the full
`pendingMint`→`mint` lifecycle, the double-mint guard, the un-pending guard, the
re-add-after-mint guard, and `onlyOperator` all pass against a **real burn proof**.
A finding fell out of it: the insecure zero-contribution ceremony's degenerate VK
`IC` points are tolerated by snarkjs/groth16-solana but **rejected by the EVM
`ecMul` precompile** — so the EVM verifier requires a non-degenerate (properly
contributed) VK; m15 uses the real PPoT ptau (which also corroborates the §B.6
ceremony tooling end-to-end). The **self-served drive is now built (P3.2–P3.5)**:
`opaq burn --submit` broadcasts the Solana burn (m16) and `evm/mint.mjs` submits the
EVM mint from the same burn proof (m17) — no relayer, both verified live. Remaining:
the `addPending` attestation (A.9 ladder) and the reverse direction (EVM→Solana).

**Trust model — see the A.9 ladder.** The EVM mint does NOT need to validate the
Solana root: the Solana `burn` instruction already enforced a valid root before
recording the nullifier, so mirroring Solana's **burned-nullifier set** (not
roots) is sufficient. `OpaqMint` takes a semi-trusted `operator` that mirrors
finalized burns as `pendingMint` entries (the ONLY trust; everything else is
ZK-bound — the proof binds the nullifier to (token, amount, dest), so the operator
attests a boolean and never sees a secret). `pendingMint` is the outstanding-burn
queue + gas refund on consume; a permanent `minted` flag is the real double-mint
guard. `operator` is **ICP-ready**: point it at an ICP canister's threshold-ECDSA
address and the operator becomes a consensus-run, on-chain, auto-signing oracle —
see A.9 for the full trust ladder up to an in-canister Solana light client. This
is where the BN254-everywhere bet pays off (one curve, one proof system).

### B.12 Phase 4 Spec — Symmetric Cross-Chain Shielded Bridge (reverse direction)

**Goal & decision.** Complete the bridge in *both* directions by making the EVM
side a **full shielded pool** (like the Solana pool), so cross-chain movement is a
private **note-burn on the source → note-mint (re-shield) on the destination**,
gated by one Groth16 proof the destination verifies. This is the symmetric design
(design "B" + re-shield): the *same* machinery runs Solana→EVM and EVM→Solana. It
**supersedes `OpaqMint`'s `balanceOf` ledger** (§6) with an EVM commitment tree,
and generalizes `burn.nr` into `xburn.nr`. Reference for the EVM pool: Tornado Cash
(a Solidity Poseidon Merkle tree + nullifier mapping — proven feasible).

**B.12.1 Note & crypto model (unchanged; shared by both chains).**
- Commitment `C = hash_4[token_id, amount, owner, blinding]`, `owner =
  hash_1[spend_key]`; nullifier `N = hash_2[C, spend_key]` — Poseidon(BN254 x5),
  identical on both chains (A.7 / M0 parity). Same as B.4 / `crates/common`.
- `token_id` is the chain-agnostic asset id: `to_field(mint)` for a Solana SPL, the
  same 32-byte field on EVM (matches OpaqMint's `bytes32 tokenId`).
- Both chains hold a depth-24 incremental Poseidon Merkle tree (ring buffer of ~32
  recent roots) + an append-only nullifier set. Solana: exists
  (`crates/common::tree`, `programs/opaq`). EVM: NEW (B.12.4).

**B.12.2 Unified circuit — `circuits/xburn/src/main.nr`** (generalizes `burn.nr`):
bind the *destination note commitment* instead of a public `dest_address`. A
cross-chain **1-in / 1-out** transfer (full amount; no change/fee in v1):

```
fn main(
    src_merkle_root: pub Field,   // a known recent root of the SOURCE tree
    src_nullifier:   pub Field,   // recorded on source; attested to the dest
    dest_chain:      pub Field,   // destination chain id (bound; can't be redirected)
    out_commitment:  pub Field,   // note to insert on the DESTINATION tree
    token_id: Field, amount: Field,               // private, conserved (in == out)
    src_spend_key: Field, src_blinding: Field,
    src_merkle_path: [Field; 24], src_merkle_path_indices: [bool; 24],
    dest_owner_pubkey: Field, dest_blinding: Field,
)
```
Constraints (mirror `burn.nr` + `transfer.nr` conservation):
1. `amount.assert_max_bit_size::<64>()` — load-bearing for soundness (B.4.1).
2. `src_owner = hash_1[src_spend_key]`; `src_commitment = hash_4[token_id, amount, src_owner, src_blinding]`.
3. Fold `src_commitment` up `src_merkle_path/indices` and assert `== src_merkle_root`.
4. `assert(hash_2[src_commitment, src_spend_key] == src_nullifier)`.
5. `assert(out_commitment == hash_4[token_id, amount, dest_owner_pubkey, dest_blinding])`
   — SAME `token_id` + `amount` ⇒ asset & value conserved across chains, both hidden.
6. `std::as_witness(dest_chain)` (keep it a bound public input).

Public inputs (4): `[src_merkle_root, src_nullifier, dest_chain, out_commitment]`;
Groth16/BN254, power 16; `nr_pubinputs = 4`.

**B.12.3 Cross-chain flow (identical both ways; A = source, B = dest).**
1. **Source `xburn` tx (A):** verify proof against a *known recent root of A* (A
   checks its own tree — no trust); reject if `src_nullifier` already spent; record
   it. No insert on A, no token release (value locked).
2. **Attestation:** operator mirrors A's finalized `src_nullifier` to B via
   `addPending(src_nullifier)` (A.9 — the ONLY trust; endgame = light client on B).
3. **Destination `mint_from_xburn` tx (B):** verify the SAME proof; require
   `pending[src_nullifier] && !minted[src_nullifier]`; require `dest_chain == B`;
   `tree_insert(out_commitment)`; set `minted[src_nullifier]`. Re-shielded on B.

B never validates A's root — mirroring the *burned-nullifier set* (not roots) is
sufficient (same argument as §6 / A.9): A's `xburn` already enforced a valid root
before recording the nullifier.

**B.12.4 EVM shielded pool — `evm/src/OpaqPool.sol`** (replaces `OpaqMint`). Tornado-
style pool speaking Opaq's note format:
- **State:** incremental Poseidon Merkle tree (depth 24, ring buffer of recent
  roots) + `mapping(bytes32=>bool) nullifierSpent | pendingMint | minted`.
  Poseidon(BN254 x5) via a Solidity/Yul impl byte-identical to light-poseidon /
  `sol_poseidon` (GATE: extend M0 parity to the EVM Poseidon — see B.12.9).
- **`xburn(a,b,c,[srcRoot,nullifier,destChain,outCommitment])`:** verify Groth16
  (existing `Groth16Verifier.sol`); require `srcRoot` is a known recent root of THIS
  pool; require `!nullifierSpent[nullifier]`; set it; emit `XBurned(...)`.
- **`addPending(bytes32 nullifier)` (onlyOperator):** mirror a finalized Solana xburn.
- **`mint_from_xburn(a,b,c,signals)`:** require `pending && !minted`; `destChain ==
  block.chainid`; verify proof; insert `outCommitment`; set `minted`.
- **`deposit(commitment)` (optional):** escrow an ERC-20 + insert a note — only for
  EVM-native assets; Solana-origin assets are fed purely by `mint_from_xburn`.

**B.12.5 Solana side.**
- **`xburn` instruction (source-side migration): DEFERRED, see P4.1's scoping
  call in B.12.8.** Originally spec'd as today's tag-4 `burn` migrated in place
  to the circuit's 4 public inputs `[merkle_root, nullifier, dest_chain,
  out_commitment]` (no token_id/amount/dest_address in the clear;
  `vk_burn`→`vk_xburn`, `nr_pubinputs=4`). Doing this before `OpaqPool.sol`
  (P4.2) exists would break the already-shipped Phase 3 forward bridge
  (m14-m18, `OpaqMint.sol`) for no working replacement — deferred until P4.2/
  P4.3 land, per B.12.7's migration note.
- **`mint_from_xburn`: DONE (P4.1, e2e m19).** Implemented as new instructions
  (tag 5 `initialize_xburn_pending`, tag 6 `add_pending_xburn`, tag 7
  `mint_from_xburn`) rather than folding into tag 4, so it ships without
  touching anything already working. Accounts `[payer(signer,w),
  commitment_tree(w), xburn_pending(w), system]`; args = proof + `[src_root,
  src_nullifier, dest_chain, out_commitment]`. Verifies Groth16 (`XBURN_VK`);
  requires `dest_chain == SOLANA_CHAIN_ID` (checked before the proof, so a
  wrong-chain submission never reaches Groth16 verification); requires
  `src_nullifier` attested pending (operator-mirrored into the `XburnPending`
  PDA via `add_pending_xburn`) and not yet minted; `tree_insert(out_commitment)`;
  marks minted. Reuses the existing verifier + `tree_insert`; `XburnPending`
  mirrors `OpaqMint.sol`'s `pendingMint`/`minted` maps (B.12.4).

**B.12.6 Attestation & trust (A.9, shared, direction-agnostic).** Each direction
mirrors the source's finalized nullifier into the destination's `pending` set via
the operator (attests a boolean, never a secret — the proof binds nullifier ↔
(token, amount, out_commitment)). `dest_chain` binding prevents redirection.
Zero-infra endgame: a light client of the *source* chain on the *destination* (A.9
rung 4) — the one real research item.

**B.12.7 Migration from OpaqMint.** Retire `OpaqMint` (balanceOf ledger); the
forward path's EVM mint changes from "credit balanceOf" to "insert out_commitment"
(`OpaqPool.mint_from_xburn`). `burn.nr`'s public `dest_address` is dropped for
`out_commitment` (recipient private on both chains). Update m15/m17/m18 to assert a
tree insert + a spendable EVM note instead of a balance.

**B.12.8 Build phases (each a commit + milestone, like P2/P3).**
- **[x] P4.0 — `xburn.nr`** + `gen_witness` fixture + Groth16 prove/verify off-chain
  (mirror P2.0/P3.0). *Accept:* proof verifies (29,476 R1CS constraints, ptau
  power 16); 4 public inputs; value conserved + amount range-checked; a
  tampered `out_commitment` fails the circuit's binding constraint.
- **[x] P4.1 — Solana `mint_from_xburn`** + `vk_xburn`; e2e **m19**
  (`scripts/m19-mint-from-xburn.sh`): an EVM-origin xburn proof (fixture)
  mints a note on Solana; double-mint, wrong `dest_chain`, and unattested
  nullifier all rejected.

  **Scoping call (deviates from this bullet's original "`xburn` migration"
  wording):** implemented `mint_from_xburn` as brand-new instructions (tag 5
  `initialize_xburn_pending`, tag 6 `add_pending_xburn`, tag 7
  `mint_from_xburn`) rather than repurposing tag 4's `burn` in place. Migrating
  tag 4 to xburn.nr's 4-public-input layout would immediately break Phase 3's
  already-shipped, already-tested forward bridge (m14-m18, `OpaqMint.sol`)
  before P4.2 (`OpaqPool.sol`) exists to replace what tag 4 currently feeds —
  i.e. it would leave the repo in a working-forward/broken-nothing-yet state
  turned into a broken-forward/nothing-working-yet state for the length of
  P4.2's build. Purely additive avoids that gap; retiring tag 4 stays exactly
  where B.12.7 already put it — after P4.2/P4.3 land and m15/17/18 are
  rewritten against `OpaqPool.sol`, not before. New state: `XburnPending` PDA
  (seed `xpending`) mirrors `OpaqMint.sol`'s `pendingMint`/`minted` maps —
  zero-copy `operator[32] | count:u64 | (nullifier[32] ‖ minted:u8)*`, same
  append-only/linear-scan/realloc shape as `NullifierSet` (B.2). `SOLANA_CHAIN_ID
  = 101` is an Opaq-internal convention (Solana has no EIP-155 chain id) that
  `xburn.nr`'s `dest_chain` public input must match when Solana is the
  destination; `gen_witness`'s xburn fixture takes `OPAQ_XBURN_DEST_CHAIN` to
  target it independently of burn's Ethereum-mainnet fixture default.
- **P4.2 — `OpaqPool.sol`** (Poseidon Merkle + nullifiers + xburn + mint_from_xburn)
  with an M0-style Poseidon parity gate (EVM == circuit == Solana); Foundry tests.
- **P4.3 — round-trip both ways, e2e m20:** Solana note → EVM note (forward,
  re-shield) and EVM note → Solana note (reverse), one proof each, live on validator
  + anvil (mirrors m18).

**B.12.9 Open questions / risks.**
- **EVM Poseidon gas:** a depth-24 Poseidon insert on EVM is expensive; benchmark
  early (Tornado's insert is ~1–2M gas — the reference budget). Do NOT swap the hash
  to save gas unless three-way parity (A.7) is preserved.
- **Poseidon parity across three impls** (Noir circuit, Solana syscall, EVM Yul) is
  the #1 risk (A.7) — extend M0's parity gate to the EVM impl before trusting it.
- **1-in/1-out only** (full-amount move, no change): partial cross-chain amounts
  need 1-in/2-out (a change note on the source) — defer to a P4.1.5 if wanted.
- **Attestation** stays semi-trusted until the light client (A.9) — unchanged from
  the forward direction.

### B.13 Viewing Keys & Received-Note Discovery (Phase 2.5)

**Problem (B.11 item #4).** A note's owner is authenticated by `spend_key`
(`owner_pubkey = Poseidon(spend_key)`, B.2) — but `spend_key` alone cannot find
which on-chain commitments belong to you. `commitment = Poseidon(token_id,
amount, owner_pubkey, blinding_factor)` is one opaque hash of four inputs; even
knowing your own `owner_pubkey`, you can't unhash a leaf to recover the
`amount`/`blinding_factor` needed to spend it, and `blinding_factor` is an
unguessable random field element. For self-created notes (`deposit`, a
`transfer`'s own change output) this is moot — the CLI already knows every
field because it generated them. It only bites for notes *someone else* sends
you (a `transfer` output to your `owner_pubkey`), which today require
out-of-band delivery of `(amount, blinding_factor)`. This section specs the
fix: an independent viewing key that lets a recipient discover incoming notes
without being told anything out-of-band.

**B.13.1 Two independent secrets, not one.** A user holds:
- `spend_key` (existing, B.2) — spend authority. `owner_pubkey =
  Poseidon(spend_key)` is permanently embedded in every commitment the user
  owns. Compromise = theft; the only remedy is racing to move funds to a fresh
  identity. **Not rotatable in place.**
- `view_key` (new) — an **independent** X25519 scalar, unrelated to
  `spend_key`/BN254. Its public half `viewing_pubkey` is published alongside
  `owner_pubkey`. Compromise = past incoming-note metadata is exposed (see
  B.13.5); **rotatable for free** — generate a new `view_key`, publish a new
  `viewing_pubkey`, `owner_pubkey`/`spend_key`/existing notes are untouched, no
  transaction required.

  `view_key` must **not** be derived from `spend_key` (e.g.
  `Poseidon(spend_key, tag)`). If it were, rotating it would be impossible
  without abandoning `owner_pubkey` too — since `owner_pubkey` is baked into
  every existing commitment, that would force moving all funds to a new
  identity just to rotate a *viewing* key. Keeping the two secrets independent
  from day one is what makes rotation cheap (B.13.5). This mirrors Zcash
  Sapling's `ak`/`nk` (spend) vs `ivk` (viewing) split.

**B.13.2 Meta-address.** A user's published receive address is the pair
`(owner_pubkey: Field, viewing_pubkey: [u8; 32])` — an X25519 public key
alongside the existing BN254 `owner_pubkey`. This is the only thing a sender
needs; `spend_key`/`view_key` themselves never leave the recipient.

**B.13.3 Sender side (`transfer` producing an output for B).** For each
non-dummy output note owned by someone else's meta-address, `opaq transfer`
additionally:
1. Generates a fresh ephemeral X25519 keypair `(esk, epk)`.
2. `shared_secret = X25519(esk, B.viewing_pubkey)`.
3. `sym_key = KDF(shared_secret)` (BLAKE3, keyed/derive_key mode — no new
   dependency family beyond what a KDF needs).
4. `plaintext = mint ‖ amount ‖ blinding_factor` (3 × 32-byte BE encodings =
   96 bytes). **`mint` is the raw SPL mint, not `token_id = to_field(mint)`**
   (B.4.2) — `to_field` is a one-way Poseidon hash, so sending only the
   field-encoded form would let the recipient recompute the commitment (both
   forms agree there) but never recover *which* mint the note is denominated
   in, making it undiscoverable-but-unspendable. `mint` is what every note
   file already stores; the recipient re-derives `to_field(mint)` locally,
   same as `deposit`/`transfer` already do. (Caught after the first pass at
   this spec shipped code with `token_id` instead — fixed before `list-unspent`
   was built on top of it.)
5. `ciphertext = ChaCha20Poly1305(sym_key, nonce, plaintext)` — reuses the
   same AEAD already used for note-at-rest encryption (B.7), just keyed via
   ECDH instead of a passphrase.
6. `memo = epk (32B) ‖ nonce (12B) ‖ ciphertext (96B + 16B tag)` = **156
   bytes** per recipient-owned output.

**B.13.4 Transport — zero circuit/ceremony impact.** The memo is **not** a
circuit input and carries no public-input weight: it's appended as a trailing,
optional section of the `transfer` instruction's own data, one memo per
non-dummy output that isn't a change-to-self. The program does not need to
parse or store it — it only reads the fixed proof + public-input prefix it
already expects; the memo simply rides along in the transaction's permanent
instruction data, retrievable by any RPC client via `getTransaction` (same
zero-infra posture as A.8, same pattern the read path already uses per M10).
Because this never touches `circuits/transfer/src/main.nr`'s public inputs,
**no new Groth16 setup or ceremony is needed** — this is purely a wallet-layer
addition on top of the existing, already-proven `transfer` circuit.

**B.13.5 Discovery (`opaq list-unspent`, B's side).**
1. Fetch all `transfer` transactions from program history
   (`getSignaturesForAddress` + `getTransaction`, per M10's read path).
2. Extract each transaction's trailing memo(s).
3. For each: `shared_secret = X25519(view_key, epk)`; attempt
   `ChaCha20Poly1305` decryption. Failure = not yours, skip (cheap; trial
   decryption over the whole history is the scan cost, same shape as Zcash's).
4. On success, recompute `commitment = Poseidon(token_id, amount, owner_pubkey,
   blinding_factor)` (using the *local* `owner_pubkey`, derived from
   `spend_key`) and check it equals one of that transaction's logged
   `out_commitment`s — defends against a malformed or misdirected memo.
5. On match: a genuine incoming note. Locate its `leaf_index` and Merkle path
   via the existing M10 `reconstruct_path`-by-commitment-value machinery
   (already built, no changes needed). Compute `nullifier =
   Poseidon(commitment, spend_key)` and check absence from the on-chain
   `NullifierSet` to confirm it's still unspent.

**Rotation bound (be honest about this):** rotating `view_key` protects only
*future* memos. Every ciphertext already posted under the old
`viewing_pubkey` is permanently public on an immutable ledger — anyone who
captured the old `view_key` before rotation can still decrypt that history
forever. This is inherent to publishing ciphertexts on-chain, not fixable
in-protocol; rotation bounds exposure going forward, it doesn't undo the past.
Treat a `view_key` leak the same way A.12 treats amount fingerprinting: a
real, named, permanent leak of *that* window's metadata, not an emergency
requiring funds to move (unlike a `spend_key` leak, which is).

**B.13.6 Scope.** v1 covers `transfer` output notes only (the B.11 #4 gap).
`xburn`'s destination note (B.12.2's `dest_owner_pubkey`/`dest_blinding`) has
the identical discovery problem on the *other* chain — the same memo scheme
should eventually extend there too — but that's follow-on work once B.12 is
built, not part of this section's scope.

**B.13.7 Build phase (P2.5).**
- **[x] P2.5.0** — `crates/common::viewkey`: X25519 keygen, ECDH, BLAKE3 KDF,
  ChaCha20Poly1305 encrypt/decrypt of the 96-byte note-opening payload,
  meta-address + `ViewKey` persistence (`to_bytes`/`from_bytes`). 7 unit
  tests: encrypt/decrypt round-trip; wrong `view_key` fails to decrypt; two
  independently-generated `view_key`s never collide; rotating `view_key`
  doesn't change `owner_pubkey`; wire-format round-trip; key persistence
  round-trip.
- **[x] P2.5.1** — sender + on-chain trailing-data plumbing:
  `opaq address --out <file>` generates a fresh `(spend_key, view_key)`
  identity and prints the public meta-address; `opaq transfer --to-view
  <hex>` encrypts out0's opening and appends the memo to the instruction
  blob (`attach_recipient_memo`, unit-tested: appends exactly `Memo::LEN`
  bytes, leaves the fixed prefix untouched, and the recipient's `view_key`
  decrypts the appended tail back to the exact opening). On-chain, `transfer`'s
  length check relaxed from `args.len() != FIXED_LEN` to `args.len() <
  FIXED_LEN` so the trailing memo doesn't get rejected — the program still
  never parses it, per B.13.4.

  Two bugs caught before P2.5.2 built on top of this: (1) `NoteOpening` was
  carrying `token_id = to_field(mint)` instead of the raw `mint` — a one-way
  hash a recipient could never invert back into "which SPL mint is this",
  making a discovered note unspendable (fixed, see B.13.3's note). (2) the
  zero-infra read path's transfer-commitment extraction sliced
  `raw.length - 64`/`raw.length - 32` — correct only when a transfer carries
  no memo; the moment one does, `raw.length` grows and those offsets slice
  into memo bytes instead of the commitments. Fixed to slice from the FIXED
  absolute offsets (353/385) and to accept `raw.length >= 417` instead of
  `=== 417` (`tests/read_path.mjs`).
- **[x] P2.5.2** — `opaq list-unspent --identity <file> --rpc --program
  --out-dir <dir>`: fetches every transfer memo + the live nullifier set
  purely over RPC (`tests/list_unspent.mjs`, zero-infra per A.8), trial-
  decrypts each with `view_key`, re-verifies the recovered opening against
  the transaction's logged `out_commitment0` (rejects a malformed/misdirected
  memo), skips anything already spent, and writes each surviving note as a
  standard (encrypted) note file `opaq withdraw` reads unmodified.
- **[x] Accept — verified on a validator (M21, `scripts/m21-view-key-
  discovery.sh` / `tests/m21_view_key_discovery.mjs`):** Alice deposits,
  generates nothing for Bob ahead of time; Bob runs `opaq address` and
  publishes his meta-address; Alice `opaq transfer --to-view`s to it; Bob,
  holding ONLY his identity file (`spend_key`, `view_key`) and zero
  out-of-band info, runs `opaq list-unspent`, discovers exactly the one note
  sent to him, and withdraws it — funds land, vault balance matches. B.11
  item #4 is closed.
