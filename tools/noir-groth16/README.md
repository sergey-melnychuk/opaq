# Noir → Groth16 backend (beta.22 port)

Phase 1's proof system is **Groth16 over BN254**, not UltraHonk — UltraHonk
verification is 3×–12× over Solana's per-tx compute budget (see M0.5 in
`OPAQ.md` B.6). `bb` does not emit Groth16, so we lower Noir's ACIR to R1CS and
prove Groth16 with snarkjs, then verify on Solana (Light Protocol's
`groth16-solana`, same `alt_bn128` syscalls).

The only existing ACIR→R1CS→Groth16 tool — [jamesbachini/Noir-Groth16] — is
pinned to **noir beta.19**. Our toolchain is **beta.22**, and the ACIR API
changed across those betas. `beta22-port.patch` forward-ports the tool:

- **`current_witness_index` removed from `acir::Circuit`** → added a
  `max_witness_index()` helper (mirrors acvm's own witness-folding) in
  `noir-acir`, used wherever the field was read (wire allocation, witness
  sizing, bounds checks).
- **`MemOp` restructured** (`operation` is now a constant `MemOpKind`,
  `index`/`value` are `Witness` not `Expression`) → fixed the remap/validation
  sites.
- **EmbeddedCurveAdd / MultiScalarMul / dynamic MemoryOp changed
  representation** (points lost the `is_infinite` coordinate; output tuples
  shrank). Opaq's circuits never emit these opcodes (Poseidon is pure
  arithmetic → `AssertZero`; fixed arrays are statically indexed). Their
  lowering is **stubbed to fail loudly** rather than risk a silent mis-lowering.
  If a future circuit needs them, port them properly first.

**Verified:** the deposit circuit (beta.22) lowers, proves, and verifies
end-to-end; snarkjs public signals match `token_id`/`amount`/`new_commitment`.

## Build

```
./setup.sh        # clones upstream @ pinned commit, applies the patch, builds noir-cli
```

Upstream pinned commit: `4b7caace1f2128e454c8d0fe50cac1ec46b1e272`.

## Caveat

This is unaudited third-party code plus our forward-port. It will need
re-porting on each Noir upgrade. Owning a minimal ACIR→R1CS for our specific
opcodes (AssertZero + RANGE) remains a cleaner long-term option if the
maintenance cost bites.

[jamesbachini/Noir-Groth16]: https://github.com/jamesbachini/Noir-Groth16
