// SPDX-License-Identifier: MIT
pragma solidity ~0.8.17;

import "forge-std/Script.sol";

interface IResolver {
    function text(bytes32 node, string calldata key) external view returns (string memory);
    function addr(bytes32 node) external view returns (address);
}

interface INameWrapper {
    function ownerOf(uint256 id) external view returns (address);
    function getData(uint256 id) external view returns (address owner, uint32 fuses, uint64 expiry);
    function names(bytes32 node) external view returns (bytes memory);
}

/// @notice Reads all data for a single subname given its label.
///
/// Computes the node from PARENT_NODE + LABEL, then reads owner info,
/// resolved address, and VPN text records.
///
/// Required env vars:
///   PARENT_NODE        - namehash of the parent domain
///   LABEL              - subdomain label (e.g. "alice" for alice.teevpn.eth)
///
/// Optional env vars:
///   PUBLIC_RESOLVER    - PublicResolver address (default: Sepolia)
///   NAME_WRAPPER       - NameWrapper address (default: Sepolia)
///
/// Usage:
///   PARENT_NODE=0x... LABEL=alice forge script script/ReadSubname.s.sol:ReadSubname --rpc-url $RPC_URL
contract ReadSubname is Script {
    address constant DEFAULT_PUBLIC_RESOLVER = 0x8FADE66B79cC9f707aB26799354482EB93a5B7dD;
    address constant DEFAULT_NAME_WRAPPER = 0x0635513f179D50A207757E05759CbD106d7dFcE8;

    function run() external view {
        bytes32 parentNode = vm.envBytes32("PARENT_NODE");
        string memory label = vm.envString("LABEL");

        address resolverAddr = vm.envOr("PUBLIC_RESOLVER", DEFAULT_PUBLIC_RESOLVER);
        address nameWrapperAddr = vm.envOr("NAME_WRAPPER", DEFAULT_NAME_WRAPPER);

        IResolver resolver = IResolver(resolverAddr);
        INameWrapper nameWrapper = INameWrapper(nameWrapperAddr);

        // Compute the subname node
        bytes32 labelHash = keccak256(bytes(label));
        bytes32 node = keccak256(abi.encodePacked(parentNode, labelHash));

        console.log("=== Subname: %s ===", label);
        console.log("LabelHash:");
        console.logBytes32(labelHash);
        console.log("Node:");
        console.logBytes32(node);

        // Owner, fuses, expiry from NameWrapper
        try nameWrapper.getData(uint256(node)) returns (address owner, uint32 fuses, uint64 expiry) {
            console.log("Owner:          ", owner);
            console.log("Fuses:          ", fuses);
            console.log("Expiry:         ", expiry);
        } catch {
            console.log("Owner:           (not wrapped / not found)");
        }

        // Resolved address
        try resolver.addr(node) returns (address resolvedAddr) {
            console.log("Addr:           ", resolvedAddr);
        } catch {
            console.log("Addr:            (not set)");
        }

        // VPN text records
        console.log("-- VPN Text Records --");
        _logText(resolver, node, "vpn.publickey");
        _logText(resolver, node, "vpn.endpoint");
        _logText(resolver, node, "vpn.http");
        _logText(resolver, node, "vpn.metadata");

        console.log("\n=== Done ===");
    }

    function _logText(IResolver resolver, bytes32 node, string memory key) internal view {
        try resolver.text(node, key) returns (string memory value) {
            if (bytes(value).length > 0) {
                console.log("  %s: %s", key, value);
            } else {
                console.log("  %s: (empty)", key);
            }
        } catch {
            console.log("  %s: (error)", key);
        }
    }
}
