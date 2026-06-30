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
        address to = address(uint160(sig[5]));

        // (1) sanity: the verifier accepts the real proof.
        require(mintc.verifier().verifyProof(a, b, c, sig), "verifier rejected a valid proof");

        // (2) mint before the operator mirrors the burn -> rejected (not pending).
        vm.expectRevert(bytes("not pending / already minted"));
        mintc.mint(a, b, c, sig);

        // (3) operator mirrors the Solana burn, then mint succeeds and credits dest.
        vm.prank(OPERATOR);
        mintc.addPending(nf);
        mintc.mint(a, b, c, sig);
        require(mintc.balanceOf(tokenId, to) == amount, "mint did not credit dest_address");
        require(mintc.minted(nf), "nullifier not marked minted");
        require(!mintc.pendingMint(nf), "pending not consumed");

        // (4) double-mint -> rejected (permanent guard).
        vm.expectRevert(bytes("not pending / already minted"));
        mintc.mint(a, b, c, sig);

        // (5) operator re-adds a consumed nullifier -> rejected (can't resurrect).
        vm.prank(OPERATOR);
        vm.expectRevert(bytes("already minted"));
        mintc.addPending(nf);
    }

    function test_onlyOperatorCanAddPending() public {
        bytes32 nf = bytes32(uint256(0x1234));
        vm.expectRevert(bytes("not operator"));
        mintc.addPending(nf);
    }
}
