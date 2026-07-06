// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

// B.12.9's EVM Poseidon parity gate — the #1 risk (A.7) for OpaqPool.sol,
// resolved as a standalone spike before OpaqPool.sol is written, mirroring
// M0's original Noir/light-poseidon/Solana-syscall spike. Every vector here
// was cross-checked light-poseidon == solana-poseidon FIRST
// (crates/common/src/lib.rs's parity_light_vs_solana /
// parity_light_vs_solana_hash1_hash4 tests) — this test extends that same
// net to the vendored Solidity implementation. If any of these fail, do NOT
// build OpaqPool.sol's tree on top of it: the roots would never match
// Solana's or the circuit's, the same "roots never match" failure family the
// B.0 parity spike exists to catch (OPAQ.md A.7).
import {PoseidonT2} from "../src/PoseidonT2.sol";
import {PoseidonT3} from "../src/PoseidonT3.sol";
import {PoseidonT5} from "../src/PoseidonT5.sol";

contract PoseidonTest {
    // hash_1 vectors (owner_pubkey = Poseidon(spend_key), B.2) — from
    // crates/common's parity_light_vs_solana_hash1_hash4.
    function test_hash1() public pure {
        require(
            PoseidonT2.hash([uint256(1)]) == 0x29176100eaa962bdc1fe6c654d6a3c130e96a4d1168b33848b897dc502820133,
            "hash1(1) mismatch"
        );
        require(
            PoseidonT2.hash([uint256(42)]) == 0x1b408dafebeddf0871388399b1e53bd065fd70f18580be5cdde15d7eb2c52743,
            "hash1(42) mismatch"
        );
        require(
            PoseidonT2.hash([uint256(987_654_321)]) == 0x127a880d2b0a0d95611d21cb836e5d458aa325f832e01146b555a95914339a43,
            "hash1(987654321) mismatch"
        );
    }

    // hash_2 vectors (nullifier = Poseidon(commitment, spend_key); to_field;
    // Merkle tree node combining, B.2/B.4.2) — from crates/common's
    // parity_light_vs_solana (== circuits/poseidon_check/src/main.nr's M0
    // vectors).
    function test_hash2() public pure {
        require(
            PoseidonT3.hash([uint256(1), uint256(2)])
                == 0x115cc0f5e7d690413df64c6b9662e9cf2a3617f2743245519e19607a4417189a,
            "hash2(1,2) mismatch"
        );
        require(
            PoseidonT3.hash([uint256(0), uint256(0)])
                == 0x2098f5fb9e239eab3ceac3f27b81e481dc3124d55ffed523a839ee8446b64864,
            "hash2(0,0) mismatch"
        );
        require(
            PoseidonT3.hash([uint256(3), uint256(4)])
                == 0x20a3af0435914ccd84b806164531b0cd36e37d4efb93efab76913a93e1f30996,
            "hash2(3,4) mismatch"
        );
        require(
            PoseidonT3.hash(
                [
                    uint256(7719472615821079694904732333912527190217998977709370935963838933860875309329),
                    uint256(15438945231642159389809464667825054380435997955418741871927677867721750618658)
                ]
            ) == 0x036e25235e4790f28f7dbed7eb3a0841726264a350565324e764beab84ba918b,
            "hash2(big,big) mismatch"
        );
    }

    // hash_4 vectors (commitment = Poseidon(token_id, amount, owner_pubkey,
    // blinding_factor), B.2) — from crates/common's
    // parity_light_vs_solana_hash1_hash4.
    function test_hash4() public pure {
        require(
            PoseidonT5.hash([uint256(1), uint256(2), uint256(3), uint256(4)])
                == 0x299c867db6c1fdd79dcefa40e4510b9837e60ebb1ce0663dbaa525df65250465,
            "hash4(1,2,3,4) mismatch"
        );
        require(
            PoseidonT5.hash([uint256(0xab), uint256(1_000_000), uint256(0xcd), uint256(123_456_789)])
                == 0x1980b19c49001e7c093efd79c709190d6404261ee9221b6530263e4d9091df39,
            "hash4(ab,1000000,cd,123456789) mismatch"
        );
    }
}
