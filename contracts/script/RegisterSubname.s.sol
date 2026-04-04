// SPDX-License-Identifier: MIT
pragma solidity ~0.8.17;

import "forge-std/Script.sol";
import {ForeverSubdomainRegistrar} from "../src/contracts/subdomainregistrar/ForeverSubdomainRegistrar.sol";

interface IResolver {
    function setText(bytes32 node, string calldata key, string calldata value) external;
}

/// @notice Registers a VPN subdomain via a deployed ForeverSubdomainRegistrar.
///         Anyone can call this — no need to be the parent name owner.
///
/// Required env vars:
///   PRIVATE_KEY        - caller private key
///   FOREVER_REGISTRAR  - deployed ForeverSubdomainRegistrar address
///   PARENT_NODE        - namehash of the parent domain (e.g. namehash("boringtun.eth"))
///   LABEL              - subdomain label (e.g. "alice" for alice.boringtun.eth)
///   PUBLIC_KEY         - WireGuard public key (base64-encoded)
///
/// Optional env vars:
///   PUBLIC_RESOLVER    - PublicResolver address (default: Sepolia)
///   OWNER              - subdomain owner address (default: caller)
///   ENDPOINT           - VPN server endpoint (ip:port)
///   HTTP_URL           - HTTP URL for the VPN node API
///   URL                - general URL for the VPN node
///   METADATA           - arbitrary metadata string (JSON recommended)
///
/// Usage:
///   forge script script/RegisterSubname.s.sol:RegisterSubname --rpc-url $RPC_URL --broadcast
contract RegisterSubname is Script {
    address constant DEFAULT_PUBLIC_RESOLVER = 0x8FADE66B79cC9f707aB26799354482EB93a5B7dD;

    function run() external {
        uint256 callerKey = vm.envUint("PRIVATE_KEY");
        address caller = vm.addr(callerKey);

        address registrarAddr = vm.envAddress("FOREVER_REGISTRAR");
        bytes32 parentNode = vm.envBytes32("PARENT_NODE");
        string memory label = vm.envString("LABEL");
        string memory publicKey = vm.envString("PUBLIC_KEY");

        address resolverAddr = vm.envOr("PUBLIC_RESOLVER", DEFAULT_PUBLIC_RESOLVER);
        address owner = vm.envOr("OWNER", caller);

        ForeverSubdomainRegistrar registrar = ForeverSubdomainRegistrar(registrarAddr);

        // Build records calldata for setText calls
        bytes32 node = keccak256(abi.encodePacked(parentNode, keccak256(bytes(label))));
        bytes[] memory records = _buildRecords(node, publicKey);

        vm.startBroadcast(callerKey);

        registrar.register(
            parentNode,
            label,
            owner,
            resolverAddr,
            0, // no extra fuses
            records
        );

        // Set optional text records directly on the resolver (caller owns the subdomain now)
        _setOptionalRecords(IResolver(resolverAddr), node);

        vm.stopBroadcast();

        console.log("\n=== Subdomain Registered ===");
        console.log("Label:       ", label);
        console.log("Owner:       ", owner);
        console.log("Public Key:  ", publicKey);
        console.log("Node:        ");
        console.logBytes32(node);
    }

    function _buildRecords(bytes32 node, string memory publicKey) internal pure returns (bytes[] memory) {
        bytes[] memory records = new bytes[](1);
        records[0] = abi.encodeWithSelector(IResolver.setText.selector, node, "vpn.publickey", publicKey);
        return records;
    }

    function _setOptionalRecords(IResolver resolver, bytes32 node) internal {
        string memory endpoint = vm.envOr("ENDPOINT", string(""));
        if (bytes(endpoint).length > 0) {
            resolver.setText(node, "vpn.endpoint", endpoint);
        }
        string memory httpUrl = vm.envOr("HTTP_URL", string(""));
        if (bytes(httpUrl).length > 0) {
            resolver.setText(node, "vpn.http", httpUrl);
        }
        string memory metadata = vm.envOr("METADATA", string(""));
        if (bytes(metadata).length > 0) {
            resolver.setText(node, "vpn.metadata", metadata);
        }
    }
}
