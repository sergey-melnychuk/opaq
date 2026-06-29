# Trusted-setup ceremony

Groth16 needs a two-phase trusted setup. Opaq splits it the standard way:

| Phase | Scope | Reused? | Why |
|-------|-------|---------|-----|
| **1 — Powers of Tau** | universal (BN254, degree only) | **yes** | circuit-independent; the expensive, hard-to-trust part — inherit a large public ceremony instead of running our own |
| **2 — per-circuit** | bound to one R1CS | **no** | must be run for *our* `deposit`/`withdraw` circuits; nothing can be inherited |

**Phase 1 is reused** from the Perpetual Powers of Tau (Polygon Hermez snapshot),
power 16 = 64k constraints — covers `withdraw` (2¹⁶) ⊇ `deposit` (2¹⁴). It is a
large, well-attended BN254 ceremony already finalized with a public beacon, so we
inherit its independent-participant trust for free. (Aztec's Ignition is the same
idea — also a BN254 Powers of Tau — but it's PLONK-only with no Groth16 phase-2 to
inherit and ships in a non-snarkjs format, so PPoT/Hermez is the turnkey source.)

**Phase 2 we run ourselves** for both circuits: ≥1 honest contributor who destroys
their toxic waste, then a public unpredictable **beacon** (default: a pinned
[drand](https://drand.love) round — verifiable threshold randomness) to finalize.

## Security model

The math needs only **one** honest contributor per phase-2 (who deletes their toxic
waste) plus a beacon nobody could predict at contribution time. Trust is therefore
**social, not in the scripts** — the tooling here is structurally correct and
auditable, but a run is only trustworthy if the contributions come from genuinely
independent parties. A run on a single machine (`--smoke`) is structurally valid but
provides ~no security and **must not** back real funds.

## Scripts

| Script | Role |
|--------|------|
| `scripts/ceremony-fetch-ptau.sh` | Phase 1: download + verify PPoT ptau (blake2b provenance + `powersoftau verify`) |
| `scripts/ceremony-phase2.sh` | Phase 2: `init` / `contribute` / `finalize`, or `local` (all-in-one) |
| `scripts/ceremony.sh` | orchestrate phase 1 + 2, embed VKs, write `transcript.md` |
| `scripts/ceremony-verify.sh` | independently re-verify ptau + both phase-2 zkeys + embedded VKs |

## Smoke (test the tooling — NOT trustworthy)

```bash
scripts/ceremony.sh --smoke     # 2 local urandom contributions + drand-latest beacon
```

Proves the pipeline yields verifying proofs. Overwrites `programs/opaq/src/vk_*.rs`
with smoke VKs; `git checkout` them afterward (the M8/M10 test path regenerates its
own throwaway VKs and is unaffected).

## Real run (coordinated, trustworthy)

1. **Pin a future drand round** `R` in advance (must be unpredictable at contribution
   time — i.e. its emission time is after the last contribution).
2. **Phase 1:** `scripts/ceremony-fetch-ptau.sh ceremony/.cache/ptau16.ptau`
3. **Phase 2, per circuit** (`deposit`, `withdraw`):
   ```bash
   # coordinator
   scripts/ceremony-phase2.sh init deposit ceremony/.cache/ptau16.ptau ceremony/work/deposit
   # each independent party, in turn (snarkjs prompts for entropy; destroy toxic waste)
   scripts/ceremony-phase2.sh contribute ceremony/work/deposit "alice"
   scripts/ceremony-phase2.sh contribute ceremony/work/deposit "bob"
   # coordinator, after round R has been emitted
   scripts/ceremony-phase2.sh finalize ceremony/work/deposit --drand-round R
   ```
4. **Embed + transcript:**
   ```bash
   scripts/ceremony.sh --real-run --drand-round R \
     --deposit-work ceremony/work/deposit --withdraw-work ceremony/work/withdraw
   ```
5. **Audit** and commit `programs/opaq/src/vk_*.rs` + `ceremony/transcript.md`:
   ```bash
   scripts/ceremony-verify.sh ceremony/.cache/ptau16.ptau \
     ceremony/work/deposit ceremony/work/withdraw
   ```

Heavy artifacts (`*.ptau`, `*.zkey`) are git-ignored — publish them to a release/IPFS;
the repo commits only the VKs and the transcript.

## Beacon: drand vs block hash

Default is **drand** (`--beacon-source drand --drand-round R`): a threshold-BLS beacon
whose output is cryptographically verifiable by re-fetching the round — scriptable, no
node required, and not grindable. A future blockchain block hash is the well-understood
alternative but a miner can grind a few bits by withholding a block, so it's offered
only as an escape hatch: `--beacon-source hex --beacon-value <64-hex>`.
