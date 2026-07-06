// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Groth16VerifierXburn} from "../src/Groth16VerifierXburn.sol";
import {OpaqPool, IGroth16VerifierXburn} from "../src/OpaqPool.sol";
import {XburnProof} from "./XburnProof.sol";

// Minimal Foundry cheatcode interface (avoids a forge-std dependency, same as
// OpaqMint.t.sol). forge runs `setUp` + any `test*` function regardless of
// inheritance.
interface Vm {
    function prank(address) external;
    function expectRevert() external;
    function expectRevert(bytes calldata) external;
    function chainId(uint256) external;
}

// OPAQ.md B.12.4/B.12.8 (Phase 4, P4.2): OpaqPool as the DESTINATION of a
// Solana-origin xburn (mintFromXburn) — the fixture is a REAL xburn.nr proof
// (evm/test/XburnProof.sol, generated from the same witness M19 used on the
// Solana side, non-degenerate PPoT so the ecMul precompile accepts it, per
// B.6/M15's finding). Covers the same 4 accept-criteria cases M19 verified
// on-chain on Solana: unattested nullifier, wrong dest_chain, happy path,
// double-mint. `xburn()`'s own happy path (EVM genuinely burning ITS OWN
// note against ITS OWN root) is exercised live in P4.3's round-trip (m20) —
// building a from-scratch witness against OpaqPool's on-chain root is that
// milestone's job, not a standalone unit test's.
contract OpaqPoolTest {
    Vm constant vm = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    OpaqPool pool;
    address constant OPERATOR = address(0xA11CE);

    function setUp() public {
        Groth16VerifierXburn verifier = new Groth16VerifierXburn();
        pool = new OpaqPool(IGroth16VerifierXburn(address(verifier)), OPERATOR);
    }

    function test_mintFromXburnLifecycle() public {
        (uint[2] memory a, uint[2][2] memory b, uint[2] memory c, uint[4] memory sig) = XburnProof.load();
        bytes32 nullifier = bytes32(sig[1]);
        bytes32 outCommitment = bytes32(sig[3]);
        // The fixture binds dest_chain = 101 (SOLANA_CHAIN_ID, programs/opaq/src/lib.rs)
        // — arbitrary from the EVM side's perspective, just needs block.chainid to match.
        uint256 destChain = sig[2];

        // (1) sanity: the verifier accepts the real proof.
        require(pool.verifier().verifyProof(a, b, c, sig), "verifier rejected a valid proof");

        // (2) mint before the operator attests it -> rejected (unattested nullifier).
        vm.chainId(destChain);
        vm.expectRevert(bytes("not pending / wrong destination"));
        pool.mintFromXburn(a, b, c, sig);

        // (3) operator attests the source-chain burn (binding the exact
        // (destChain, outCommitment) tuple, not just the nullifier — B.12.5);
        // wrong dest_chain -> rejected.
        vm.prank(OPERATOR);
        pool.addPending(nullifier, destChain, uint256(outCommitment));
        vm.chainId(destChain + 1);
        vm.expectRevert(bytes("wrong dest chain"));
        pool.mintFromXburn(a, b, c, sig);

        // (3b) a proof claiming the RIGHT nullifier but a DIFFERENT
        // out_commitment than what was attested -> rejected. This is the
        // tuple-binding check itself: a bare nullifier flag would have let
        // this through (found while scoping the ICP attestor, B.14.7).
        vm.chainId(destChain);
        uint256[4] memory tamperedSig = [sig[0], sig[1], sig[2], sig[3] + 1];
        vm.expectRevert(bytes("not pending / wrong destination"));
        pool.mintFromXburn(a, b, c, tamperedSig);

        // (4) correct chain, matching tuple -> mint succeeds: re-shields
        // outCommitment as leaf 0.
        pool.mintFromXburn(a, b, c, sig);
        require(pool.minted(nullifier), "nullifier not marked minted");
        require(pool.pendingMint(nullifier) == bytes32(0), "pending not consumed");
        require(pool.nextIndex() == 1, "outCommitment should be inserted as leaf 0");
        require(pool.roots(pool.currentRootIndex()) != 0, "root should have moved off genesis");

        // (5) double-mint -> rejected (permanent guard; checked before the
        // pending/tuple check, since pendingMint was already deleted above).
        vm.expectRevert(bytes("already minted"));
        pool.mintFromXburn(a, b, c, sig);

        // (6) operator re-adds a consumed nullifier -> rejected (can't resurrect).
        vm.prank(OPERATOR);
        vm.expectRevert(bytes("already minted"));
        pool.addPending(nullifier, destChain, uint256(outCommitment));
    }

    function test_onlyOperatorCanAddPending() public {
        vm.expectRevert(bytes("not operator"));
        pool.addPending(bytes32(uint256(0x1234)), 1, 2);
    }

    // xburn() as the SOURCE: the cheap root-membership check must reject an
    // unknown root before ever reaching the expensive pairing check — proven
    // here with a garbage root and a zeroed dummy proof (which would also
    // fail verification, but the revert reason confirms it never got there).
    function test_xburnUnknownRootRejected() public {
        uint[2] memory zeroA;
        uint[2][2] memory zeroB;
        uint[2] memory zeroC;
        uint256[4] memory signals = [uint256(0xdeadbeef), uint256(1), uint256(1), uint256(2)];
        vm.expectRevert(bytes("unknown root"));
        pool.xburn(zeroA, zeroB, zeroC, signals);
    }
}
