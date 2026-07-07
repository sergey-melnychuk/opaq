# Trusted-setup ceremony

Groth16 needs a two-phase trusted setup. Opaq splits it the standard way:

| Phase | Scope | Reused? | Why |
|-------|-------|---------|-----|
| **1 — Powers of Tau** | universal (BN254, degree only) | **yes** | circuit-independent; the expensive, hard-to-trust part — inherit a large public ceremony instead of running our own |
| **2 — per-circuit** | bound to one R1CS | **no** | must be run for *each* circuit (`deposit`, `withdraw`, `transfer`, `burn`, `xburn`); nothing can be inherited |

**Phase 1 is reused** from the Perpetual Powers of Tau (Polygon Hermez snapshot),
power 16 = 64k constraints — covers every current circuit (`withdraw`/`burn`/`xburn`
at 2¹⁶, `deposit` at 2¹⁴, `transfer` needs power 17 separately — see the per-circuit
power column in `scripts/ceremony.sh`). It is a large, well-attended BN254 ceremony
already finalized with a public beacon, so we inherit its independent-participant
trust for free. (Aztec's Ignition is the same idea — also a BN254 Powers of Tau —
but it's PLONK-only with no Groth16 phase-2 to inherit and ships in a non-snarkjs
format, so PPoT/Hermez is the turnkey source. PLONK/KZG-based systems use this same
kind of powers-of-tau SRS *directly*, with no circuit-specific phase 2 at all — that
asymmetry is *why* Groth16 needs a phase 2 per circuit while phase 1 is shared,
reusable infrastructure regardless of which downstream proof system consumes it.)

**Phase 2 we run ourselves** per circuit: ≥1 honest contributor who destroys
their toxic waste, then a public unpredictable **beacon** (default: a pinned
[drand](https://drand.love) round — verifiable threshold randomness) to finalize.

## What "toxic waste" is, and why one honest deletion is enough

Groth16's setup builds the proving/verifying key pair from secret random field
elements. Nobody needs them again once the setup is done — *unless* they're
trying to cheat: knowing them lets you construct a proof that verifies for a
**false** statement, indistinguishable on-chain from a real one. That's the
entire risk toxic waste refers to — a literal secret that breaks soundness if
it survives anywhere, not a metaphor for sloppy process.

A multi-party ceremony needs only **one** honest participant because
contributions compose multiplicatively: each party takes the accumulated
secret, multiplies in their own fresh randomness, passes it on, then deletes
their own piece. The final secret is the product of every piece. If even one
piece was genuinely never persisted — not on disk, not in process memory, not
logged — nobody can reconstruct the final secret afterward, even by later
compromising every other contributor, because reconstruction needs *every*
piece and one is provably gone. Hence 1-of-n honesty, not unanimity.

A *trivial* ceremony (no contribution, or fixed-string entropy — which is
exactly what `scripts/groth16-setup.sh` uses for testing today, `-e="opaq
deterministic entropy"`) isn't merely weak, it's catastrophic: the secret
isn't hard to guess, it's **published in this repository**. Anyone who clones
it can recompute it and forge arbitrary proofs — no cryptography required,
just reading the source.

A drand beacon as the *final* contribution step helps even a lone operator:
its value is unpredictable *at contribution time* (published by an
independent network afterward), so the operator couldn't have steered the
final key toward a chosen backdoor. It does **not** prove they genuinely
destroyed their *own* earlier-round randomness instead of retaining it —
that's fundamentally unverifiable from outside a single-operator process,
which is exactly why a solo run below is labeled "not trustworthy for real
funds" even when it uses real entropy and a real beacon correctly.

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
| `scripts/ceremony-fetch-ptau.sh` | Phase 1: download + verify PPoT ptau for a given power (blake2b provenance + `powersoftau verify`) |
| `scripts/ceremony-phase2.sh` | Phase 2: `init` / `contribute` / `finalize`, or `local` (all-in-one), for any one circuit |
| `scripts/ceremony.sh` | orchestrate phase 1 + 2 for **all 5 circuits**, embed VKs, write `transcript.md` |
| `scripts/ceremony-verify.sh` | independently re-verify every circuit's ptau + phase-2 zkey + embedded VK |

All 5 circuits (`deposit`, `withdraw`, `transfer`, `burn`, `xburn`) need their own
phase-2 — `transfer` needs ptau power 17 (128k constraints), the other four share
power 16; `ceremony.sh` fetches both automatically.

## Smoke (test the tooling — NOT trustworthy)

```bash
scripts/ceremony.sh --smoke     # 2 local urandom contributions + drand-latest beacon, all 5 circuits
```

Proves the pipeline yields verifying proofs, and replaces whatever's currently
embedded (including `groth16-setup.sh`'s hardcoded-entropy test zkeys — see
"What toxic waste is" above) with keys nobody could have pre-computed a specific
backdoor into. Still not trustworthy for real funds (one machine saw every
round) — `programs/opaq/src/vk_*.rs` should say so in the commit that embeds
smoke output, not be mistaken for a real ceremony's result.

## Real run (coordinated, trustworthy)

1. **Pin a future drand round** `R` in advance (must be unpredictable at contribution
   time — i.e. its emission time is after the last contribution).
2. **Phase 1:** `scripts/ceremony-fetch-ptau.sh ceremony/.cache/ptau16.ptau 16` and,
   for `transfer`, `scripts/ceremony-fetch-ptau.sh ceremony/.cache/ptau17.ptau 17`.
3. **Phase 2, per circuit** (repeat for `deposit`, `withdraw`, `transfer`, `burn`, `xburn`
   — use `ptau17.ptau` for `transfer`, `ptau16.ptau` for the rest):
   ```bash
   # coordinator
   scripts/ceremony-phase2.sh init deposit ceremony/.cache/ptau16.ptau ceremony/work/deposit
   # each independent party, in turn (snarkjs prompts for entropy; destroy toxic waste)
   scripts/ceremony-phase2.sh contribute ceremony/work/deposit "alice"
   scripts/ceremony-phase2.sh contribute ceremony/work/deposit "bob"
   # coordinator, after round R has been emitted
   scripts/ceremony-phase2.sh finalize ceremony/work/deposit --drand-round R
   ```
4. **Embed + transcript** (one `--work` per circuit):
   ```bash
   scripts/ceremony.sh --real-run --drand-round R \
     --work deposit:ceremony/work/deposit --work withdraw:ceremony/work/withdraw \
     --work transfer:ceremony/work/transfer --work burn:ceremony/work/burn \
     --work xburn:ceremony/work/xburn
   ```
5. **Audit** and commit `programs/opaq/src/vk_*.rs` + `ceremony/transcript.md`:
   ```bash
   scripts/ceremony-verify.sh \
     deposit:ceremony/work/deposit withdraw:ceremony/work/withdraw \
     transfer:ceremony/work/transfer burn:ceremony/work/burn xburn:ceremony/work/xburn
   ```

Heavy artifacts (`*.ptau`, `*.zkey`) are git-ignored — publish them to a release/IPFS;
the repo commits only the VKs and the transcript.

## Beacon: drand vs block hash

Default is **drand** (`--beacon-source drand --drand-round R`): a threshold-BLS beacon
whose output is cryptographically verifiable by re-fetching the round — scriptable, no
node required, and not grindable. A future blockchain block hash is the well-understood
alternative but a miner can grind a few bits by withholding a block, so it's offered
only as an escape hatch: `--beacon-source hex --beacon-value <64-hex>`.
