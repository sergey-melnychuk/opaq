// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

// Opaq cross-chain mint (OPAQ.md A.6 Phase 3, EVM side).
//
// Mints the EVM counterpart of a note BURNED on Solana. The burn proof's public
// signals are, in burn.nr order:
//   [0] merkle_root  [1] nullifier  [2] token_id  [3] amount
//   [4] dest_chain   [5] dest_address
//
// Trust model (A.9): a semi-trusted operator mirrors Solana's FINALIZED burns
// (its nullifier set) as `pendingMint` entries. Because the Solana `burn`
// instruction already enforces a valid on-chain root before recording a
// nullifier, a mirrored burn implies "real note, real root, really burned" — so
// the EVM needs no root validation of its own. Everything else is ZK-bound: the
// proof binds the nullifier to (token, amount, dest_address), so the operator
// attests only a boolean per burn and never touches amounts or destinations.
//
// `pendingMint` is the outstanding-burn queue (and a gas refund on consume);
// `minted` is the permanent double-mint guard. The operator can only ADD pending
// (never remove — removal happens solely by minting), and cannot resurrect a
// consumed burn. Residual trust: the operator won't fabricate a burn that didn't
// happen — exactly what a Solana light client would later remove.
interface IGroth16Verifier {
    function verifyProof(
        uint[2] calldata a,
        uint[2][2] calldata b,
        uint[2] calldata c,
        uint[6] calldata signals
    ) external view returns (bool);
}

contract OpaqMint {
    IGroth16Verifier public immutable verifier;
    address public operator;

    mapping(bytes32 => bool) public pendingMint; // operator-mirrored, awaiting mint
    mapping(bytes32 => bool) public minted; // permanent double-mint guard
    // Demo asset ledger: token_id => holder => amount (a real deployment would
    // mint a per-token_id ERC-20; kept self-contained here).
    mapping(bytes32 => mapping(address => uint256)) public balanceOf;

    event PendingAdded(bytes32 indexed nullifier);
    event Minted(bytes32 indexed nullifier, bytes32 indexed tokenId, address indexed to, uint256 amount);

    modifier onlyOperator() {
        require(msg.sender == operator, "not operator");
        _;
    }

    constructor(IGroth16Verifier _verifier, address _operator) {
        verifier = _verifier;
        operator = _operator;
    }

    /// Operator mirrors a FINALIZED Solana burn. Refuses to resurrect a consumed
    /// nullifier, so a re-add can never enable a second mint.
    function addPending(bytes32 nullifier) external onlyOperator {
        require(!minted[nullifier], "already minted");
        pendingMint[nullifier] = true;
        emit PendingAdded(nullifier);
    }

    /// Mint the burned note's EVM counterpart. Anyone can submit, but only the
    /// note owner can produce a valid proof revealing this nullifier.
    function mint(
        uint[2] calldata a,
        uint[2][2] calldata b,
        uint[2] calldata c,
        uint[6] calldata signals
    ) external {
        bytes32 nullifier = bytes32(signals[1]);
        require(pendingMint[nullifier] && !minted[nullifier], "not pending / already minted");
        require(signals[4] == block.chainid, "wrong dest chain");
        require(verifier.verifyProof(a, b, c, signals), "bad proof");

        bytes32 tokenId = bytes32(signals[2]);
        uint256 amount = signals[3];
        address to = address(uint160(signals[5])); // dest_address field -> 20-byte addr

        minted[nullifier] = true; // permanent guard
        delete pendingMint[nullifier]; // consume (gas refund + accurate outstanding set)
        balanceOf[tokenId][to] += amount;
        emit Minted(nullifier, tokenId, to, amount);
    }
}
