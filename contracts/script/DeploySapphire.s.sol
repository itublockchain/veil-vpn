// SPDX-License-Identifier: MIT
pragma solidity ^0.8.17;

import "forge-std/Script.sol";
import {TEEAttestationRegistry} from "../src/tee/TEEAttestationRegistry.sol";

/// @notice Deploys TEEAttestationRegistry on Oasis Sapphire.
///
/// Usage:
///   export PRIVATE_KEY=0x...
///   export ROFL_APP_ID=0x...  (21-byte ROFL app ID, hex-encoded)
///   forge script script/DeploySapphire.s.sol:DeploySapphire \
///     --rpc-url $SAPPHIRE_RPC_URL \
///     --broadcast
contract DeploySapphire is Script {
    function run() external {
        uint256 deployerKey = vm.envUint("PRIVATE_KEY");
        bytes21 roflAppId = bytes21(vm.envBytes("ROFL_APP_ID"));

        vm.startBroadcast(deployerKey);

        TEEAttestationRegistry registry = new TEEAttestationRegistry(roflAppId);

        vm.stopBroadcast();

        console.log("\n=== Deployment Summary (Sapphire) ===");
        console.log("TEEAttestationRegistry:", address(registry));
        console.log("ROFL App ID:");
        console.logBytes(abi.encodePacked(roflAppId));
    }
}
