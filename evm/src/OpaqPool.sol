// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

// Opaq shielded pool — EVM side of the symmetric cross-chain bridge (OPAQ.md
// B.12.4, Phase 4). Replaces OpaqMint's balanceOf ledger: instead of crediting
// a balance, a Solana-origin xburn re-shields directly into THIS pool's own
// Poseidon Merkle tree — the same tree/note format `programs/opaq` uses
// (B.12.1), so the destination note is immediately a normal, spendable Opaq
// note on this chain, not an IOU.
//
// Two independent directions share the same tree + verifier:
//   - `xburn`: THIS pool is the SOURCE. Verifies a proof against ITS OWN known
//     recent root, checks the nullifier hasn't been spent, records it. No
//     insert here — the note is now claimable on the destination via the
//     same proof (mirrors programs/opaq's `burn`/`xburn` shape).
//   - `mintFromXburn`: THIS pool is the DESTINATION. Verifies the SAME proof
//     a source chain's xburn verified, requires the operator has attested
//     `srcNullifier` as pending-and-unminted, inserts `outCommitment`.
//     Never validates `srcRoot` — it isn't this chain's root to check
//     (B.12.3): the source chain already enforced a valid root before
//     recording its own nullifier.
//
// Circuit ABI (xburn.nr, B.12.2), same order both directions:
//   signals[0] = srcRoot  signals[1] = srcNullifier
//   signals[2] = destChain  signals[3] = outCommitment
// token_id/amount are private circuit witnesses (conserved, never public) —
// this contract never sees them, same as programs/opaq's mint_from_xburn.
//
// Trust model (A.9, shared with OpaqMint): a semi-trusted `operator` mirrors
// FINALIZED source-chain burns into `pendingMint`. The proof itself binds the
// nullifier to (token, amount, destination note), so the operator attests
// only a boolean and never touches a secret.
import {PoseidonT3} from "./PoseidonT3.sol";

interface IGroth16VerifierXburn {
    function verifyProof(
        uint[2] calldata a,
        uint[2][2] calldata b,
        uint[2] calldata c,
        uint[4] calldata signals
    ) external view returns (bool);
}

contract OpaqPool {
    uint256 constant TREE_DEPTH = 24;
    uint256 constant ROOT_HISTORY = 32;

    IGroth16VerifierXburn public immutable verifier;
    address public operator;

    uint256 public nextIndex;
    uint256 public currentRootIndex;
    uint256[TREE_DEPTH] public filledSubtrees;
    uint256[ROOT_HISTORY] public roots;
    // Empty-subtree hashes at each level — same table as
    // programs/opaq/src/tree_consts.rs::ZEROS, reused (not recomputed) here
    // so both trees start from byte-identical empty-tree state (A.7).
    uint256[TREE_DEPTH] public zeros;

    mapping(bytes32 => bool) public nullifierSpent; // this pool's OWN xburn nullifiers (EVM as SOURCE)
    // operator-attested source-chain xburns (EVM as DEST): nullifier => hash
    // of the SPECIFIC (destChain, outCommitment) attested, 0 = not pending.
    // Bare bool here would let mintFromXburn accept ANY proof sharing this
    // nullifier, not just the one actually attested — destChain/outCommitment
    // are free choices at proof-generation time, unconstrained by the note
    // itself, so a bare flag can't tell which destination was really burned
    // for (found while scoping the ICP attestor, see OPAQ.md B.14.7).
    mapping(bytes32 => bytes32) public pendingMint;
    mapping(bytes32 => bool) public minted; // permanent double-mint guard

    event XBurned(bytes32 indexed nullifier, bytes32 destChain, bytes32 outCommitment);
    event PendingAdded(bytes32 indexed nullifier, uint256 destChain, uint256 outCommitment);
    event Minted(bytes32 indexed nullifier, bytes32 indexed outCommitment, uint256 leafIndex);

    modifier onlyOperator() {
        require(msg.sender == operator, "not operator");
        _;
    }

    constructor(IGroth16VerifierXburn _verifier, address _operator) {
        verifier = _verifier;
        operator = _operator;

        // programs/opaq/src/tree_consts.rs::ZEROS, verbatim (B.12.9 gate:
        // evm/test/Poseidon.t.sol already proved PoseidonT3 byte-matches the
        // same Poseidon these were generated with).
        uint256[TREE_DEPTH] memory z = [
            uint256(0x0000000000000000000000000000000000000000000000000000000000000000),
            0x2098f5fb9e239eab3ceac3f27b81e481dc3124d55ffed523a839ee8446b64864,
            0x1069673dcdb12263df301a6ff584a7ec261a44cb9dc68df067a4774460b1f1e1,
            0x18f43331537ee2af2e3d758d50f72106467c6eea50371dd528d57eb2b856d238,
            0x07f9d837cb17b0d36320ffe93ba52345f1b728571a568265caac97559dbc952a,
            0x2b94cf5e8746b3f5c9631f4c5df32907a699c58c94b2ad4d7b5cec1639183f55,
            0x2dee93c5a666459646ea7d22cca9e1bcfed71e6951b953611d11dda32ea09d78,
            0x078295e5a22b84e982cf601eb639597b8b0515a88cb5ac7fa8a4aabe3c87349d,
            0x2fa5e5f18f6027a6501bec864564472a616b2e274a41211a444cbe3a99f3cc61,
            0x0e884376d0d8fd21ecb780389e941f66e45e7acce3e228ab3e2156a614fcd747,
            0x1b7201da72494f1e28717ad1a52eb469f95892f957713533de6175e5da190af2,
            0x1f8d8822725e36385200c0b201249819a6e6e1e4650808b5bebc6bface7d7636,
            0x2c5d82f66c914bafb9701589ba8cfcfb6162b0a12acf88a8d0879a0471b5f85a,
            0x14c54148a0940bb820957f5adf3fa1134ef5c4aaa113f4646458f270e0bfbfd0,
            0x190d33b12f986f961e10c0ee44d8b9af11be25588cad89d416118e4bf4ebe80c,
            0x22f98aa9ce704152ac17354914ad73ed1167ae6596af510aa5b3649325e06c92,
            0x2a7c7c9b6ce5880b9f6f228d72bf6a575a526f29c66ecceef8b753d38bba7323,
            0x2e8186e558698ec1c67af9c14d463ffc470043c9c2988b954d75dd643f36b992,
            0x0f57c5571e9a4eab49e2c8cf050dae948aef6ead647392273546249d1c1ff10f,
            0x1830ee67b5fb554ad5f63d4388800e1cfe78e310697d46e43c9ce36134f72cca,
            0x2134e76ac5d21aab186c2be1dd8f84ee880a1e46eaf712f9d371b6df22191f3e,
            0x19df90ec844ebc4ffeebd866f33859b0c051d8c958ee3aa88f8f8df3db91a5b1,
            0x18cca2a66b5c0787981e69aefd84852d74af0e93ef4912b4648c05f722efe52b,
            0x2388909415230d1b4d1304d2d54f473a628338f2efad83fadf05644549d2538d
        ];
        for (uint256 i = 0; i < TREE_DEPTH; i++) {
            zeros[i] = z[i];
            filledSubtrees[i] = z[i];
        }
        // Genesis root == programs/opaq/src/tree_consts.rs::EMPTY_ROOT.
        roots[0] = 0x27171fb4a97b6cc0e9e8f543b5294de866a2af2c9c8d0b1d96e673e4529ed540;
    }

    /// Zero-copy-equivalent incremental insert (mirrors programs/opaq's
    /// tree_insert exactly: same fold direction, same ring-buffer write).
    function _insert(uint256 leaf) internal returns (uint256 leafIndex) {
        uint256 idx = nextIndex;
        require(idx < (1 << TREE_DEPTH), "tree full");
        leafIndex = idx;

        uint256 current = leaf;
        for (uint256 i = 0; i < TREE_DEPTH; i++) {
            uint256 left;
            uint256 right;
            if (idx & 1 == 0) {
                filledSubtrees[i] = current;
                left = current;
                right = zeros[i];
            } else {
                left = filledSubtrees[i];
                right = current;
            }
            current = PoseidonT3.hash([left, right]);
            idx >>= 1;
        }

        currentRootIndex = (currentRootIndex + 1) % ROOT_HISTORY;
        roots[currentRootIndex] = current;
        nextIndex = leafIndex + 1;
    }

    function _isKnownRoot(uint256 root) internal view returns (bool) {
        if (root == 0) return false;
        for (uint256 i = 0; i < ROOT_HISTORY; i++) {
            if (roots[i] == root) return true;
        }
        return false;
    }

    /// THIS pool as the SOURCE: burn a local note for a cross-chain move.
    /// Cheap checks before the expensive pairing check (gas-order, unlike
    /// programs/opaq's compute-unit model where this reordering doesn't pay
    /// off the same way).
    function xburn(uint[2] calldata a, uint[2][2] calldata b, uint[2] calldata c, uint256[4] calldata signals)
        external
    {
        bytes32 nullifier = bytes32(signals[1]);
        require(_isKnownRoot(signals[0]), "unknown root");
        require(!nullifierSpent[nullifier], "already spent");
        require(verifier.verifyProof(a, b, c, signals), "bad proof");

        nullifierSpent[nullifier] = true;
        emit XBurned(nullifier, bytes32(signals[2]), bytes32(signals[3]));
    }

    /// Operator mirrors a FINALIZED source-chain xburn (A.9 — the only trust;
    /// the proof itself binds nullifier <-> token/amount/destination note).
    /// Binds the SPECIFIC (destChain, outCommitment) attested, not just the
    /// nullifier — see the `pendingMint` doc comment. Refuses to resurrect a
    /// consumed nullifier, mirroring OpaqMint.sol.
    function addPending(bytes32 nullifier, uint256 destChain, uint256 outCommitment) external onlyOperator {
        require(!minted[nullifier], "already minted");
        pendingMint[nullifier] = keccak256(abi.encode(destChain, outCommitment));
        emit PendingAdded(nullifier, destChain, outCommitment);
    }

    /// THIS pool as the DESTINATION: re-shield a source-chain burn by
    /// inserting its `outCommitment` — a real spendable note here, not a
    /// balance credit (this is what supersedes OpaqMint.mint, B.12.7).
    function mintFromXburn(uint[2] calldata a, uint[2][2] calldata b, uint[2] calldata c, uint256[4] calldata signals)
        external
    {
        bytes32 nullifier = bytes32(signals[1]);
        require(!minted[nullifier], "already minted");
        // Check `minted` first (above): once consumed, pendingMint[nullifier]
        // is deleted, so a stale re-submission must fail as "already minted",
        // not fall through to "not pending" — order matters here.
        bytes32 expected = keccak256(abi.encode(signals[2], signals[3]));
        require(pendingMint[nullifier] == expected && expected != bytes32(0), "not pending / wrong destination");
        require(signals[2] == block.chainid, "wrong dest chain");
        require(verifier.verifyProof(a, b, c, signals), "bad proof");

        minted[nullifier] = true; // permanent guard
        delete pendingMint[nullifier]; // consume (gas refund + accurate outstanding set)
        uint256 leafIndex = _insert(signals[3]);
        emit Minted(nullifier, bytes32(signals[3]), leafIndex);
    }
}
