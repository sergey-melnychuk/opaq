// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Groth16Verifier} from "../src/Groth16Verifier.sol";
import {OpaqMint, IGroth16Verifier} from "../src/OpaqMint.sol";
import {BurnProof} from "./BurnProof.sol";

// Minimal Foundry cheatcode interface (avoids a forge-std dependency). forge runs
// `setUp` + any `test*` function regardless of inheritance.
interface Vm {
    function prank(address) external;
    function expectRevert() external;
    function expectRevert(bytes calldata) external;
    function chainId(uint256) external;
}

contract OpaqMintTest {
    Vm constant vm = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    OpaqMint mintc;
    address constant OPERATOR = address(0xA11CE);

    function setUp() public {
        // The burn fixture binds dest_chain = 1; make block.chainid match.
        vm.chainId(1);
        Groth16Verifier verifier = new Groth16Verifier();
        mintc = new OpaqMint(IGroth16Verifier(address(verifier)), OPERATOR);
    }

    function test_mintLifecycle() public {
        (uint[2] memory a, uint[2][2] memory b, uint[2] memory c, uint[6] memory sig) = BurnProof.load();
        bytes32 nf = bytes32(sig[1]);
        bytes32 tokenId = bytes32(sig[2]);
        uint256 amount = sig[3];
        uint256 destChain = sig[4];
        address to = address(uint160(sig[5]));

        // (1) sanity: the verifier accepts the real proof.
        require(mintc.verifier().verifyProof(a, b, c, sig), "verifier rejected a valid proof");

        // (2) mint before the operator mirrors the burn -> rejected (not pending).
        vm.expectRevert(bytes("not pending / wrong destination"));
        mintc.mint(a, b, c, sig);

        // (3) operator mirrors the Solana burn, binding the exact
        // (tokenId, amount, destChain, destAddress) tuple (B.12.5's fix,
        // ported here after finding the identical gap in OpaqPool.sol).
        vm.prank(OPERATOR);
        mintc.addPending(nf, tokenId, amount, destChain, to);

        // (3b) a proof claiming the right nullifier but a DIFFERENT
        // dest_address than what was attested -> rejected. Bare nullifier
        // tracking (the original design) would have let this through.
        uint[6] memory tampered = [sig[0], sig[1], sig[2], sig[3], sig[4], uint256(uint160(to)) ^ 1];
        vm.expectRevert(bytes("not pending / wrong destination"));
        mintc.mint(a, b, c, tampered);

        // (4) matching tuple -> mint succeeds and credits dest.
        mintc.mint(a, b, c, sig);
        require(mintc.balanceOf(tokenId, to) == amount, "mint did not credit dest_address");
        require(mintc.minted(nf), "nullifier not marked minted");
        require(mintc.pendingMint(nf) == bytes32(0), "pending not consumed");

        // (5) double-mint -> rejected (permanent guard; checked before the
        // pending/tuple check, since pendingMint was already deleted above).
        vm.expectRevert(bytes("already minted"));
        mintc.mint(a, b, c, sig);

        // (6) operator re-adds a consumed nullifier -> rejected (can't resurrect).
        vm.prank(OPERATOR);
        vm.expectRevert(bytes("already minted"));
        mintc.addPending(nf, tokenId, amount, destChain, to);
    }

    function test_onlyOperatorCanAddPending() public {
        bytes32 nf = bytes32(uint256(0x1234));
        vm.expectRevert(bytes("not operator"));
        mintc.addPending(nf, bytes32(uint256(1)), 1, 1, address(0xBEEF));
    }
}
