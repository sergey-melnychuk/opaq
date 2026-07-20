# opaq-attestor

Opaq's ICP attestation canister (OPAQ.md B.14, Phase 5, A.9 rung 2) —
threshold-signing replacement for the single-operator bridge key. Scaffolded
via `icp-cli`'s `rust` recipe (`icp.yaml`); see OPAQ.md B.14 for the full
design and B.14.6 for build-phase status.

## Layout

- `src/lib.rs` — P5.0: threshold-ECDSA (EVM) / threshold-Ed25519 (Solana)
  key derivation and sign+verify self-test (`evm_address`, `solana_address`,
  `evm_sign_and_verify`, `solana_sign_and_verify`).
- `src/evm.rs` / `src/evm_tx.rs` — P5.1: EVM leg via `evm-rpc-canister`
  (`check_nullifier_spent`, `read_pending_mint`, `submit_add_pending`).
- `src/solana.rs` / `src/solana_tx.rs` — P5.2: Solana leg via raw HTTP
  outcalls (`verify_xburn_transaction`, `submit_add_pending_xburn`).

This crate is an isolated `[workspace]` (own `Cargo.lock`) — same reason as
`programs/poseidon-syscall-check`: its `wasm32-unknown-unknown` dependency
graph doesn't belong in the main repo workspace.

## Local network

```bash
icp network start -d   # gateway on port 8010, not the 8000 default — see icp.yaml
icp deploy --yes
icp canister call opaq_attestor evm_address '()'
icp network stop
```

`icp.yaml`'s `local` environment also deploys a local copy of DFINITY's
`evm-rpc-canister` (needed only for local dev — mainnet uses the shared
canister at `7hfb6-caaaa-aaaar-qadga-cai`, see `icp.yaml`'s `ic` environment).

For live fixtures to test the EVM/Solana legs against real local chain state,
see `scripts/p5.2-solana-fixture.sh` (Solana) and the P5.1 verification notes
in OPAQ.md B.14.6 (anvil + a redeployed `OpaqPool` with this canister's own
derived address as `operator`).
