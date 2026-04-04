// SPDX-License-Identifier: MIT
pragma solidity ^0.8.17;

import {Subcall} from "@oasisprotocol/sapphire-contracts/contracts/Subcall.sol";

/// @title TEEAttestationRegistry
/// @notice Stores attestation records for VPN nodes on Oasis Sapphire.
///         Only authorized ROFL app instances can register attestations.
///         Clients use this to verify that a node was registered from a real TEE.
contract TEEAttestationRegistry {
    struct Attestation {
        address attester;
        string label;
        uint256 timestamp;
    }

    /// @notice The authorized ROFL app ID (set at deploy time).
    bytes21 public roflAppId;

    /// @notice ensNode → Attestation
    mapping(bytes32 => Attestation) private attestations;

    /// @notice All registered ensNode hashes (for enumeration).
    bytes32[] private subnameNodes;

    /// @notice Track which ensNodes are already registered (avoid duplicates in array).
    mapping(bytes32 => bool) private registered;

    event AttestationRegistered(
        bytes32 indexed ensNode,
        string label,
        address attester,
        uint256 timestamp
    );

    constructor(bytes21 _roflAppId) {
        roflAppId = _roflAppId;
    }

    /// @notice Register an attestation for an ENS subname node.
    ///         Caller must be an authorized ROFL app instance — verified on-chain
    ///         via Sapphire's Subcall precompile.
    /// @param ensNode The namehash of the ENS subname.
    /// @param label The human-readable label of the subname.
    function register(bytes32 ensNode, string calldata label) external {
        Subcall.roflEnsureAuthorizedOrigin(roflAppId);

        attestations[ensNode] = Attestation({
            attester: msg.sender,
            label: label,
            timestamp: block.timestamp
        });

        if (!registered[ensNode]) {
            registered[ensNode] = true;
            subnameNodes.push(ensNode);
        }

        emit AttestationRegistered(ensNode, label, msg.sender, block.timestamp);
    }

    /// @notice Get the attestation for an ENS node.
    function getAttestation(bytes32 ensNode)
        external
        view
        returns (address attester, string memory label, uint256 timestamp)
    {
        Attestation storage a = attestations[ensNode];
        return (a.attester, a.label, a.timestamp);
    }

    /// @notice Get all attested ENS node hashes.
    function getSubnames() external view returns (bytes32[] memory) {
        return subnameNodes;
    }

    /// @notice Check if an ENS node has a valid attestation.
    function isAttested(bytes32 ensNode) external view returns (bool) {
        return registered[ensNode];
    }
}
