// SPDX-License-Identifier: MIT
pragma solidity ~0.8.17;

import "forge-std/Script.sol";
import {INameWrapper, CANNOT_UNWRAP} from "../src/contracts/wrapper/INameWrapper.sol";
import {IBaseRegistrar} from "../src/contracts/ethregistrar/IBaseRegistrar.sol";
import {ForeverSubdomainRegistrar} from "../src/contracts/subdomainregistrar/ForeverSubdomainRegistrar.sol";
import {FixedPricer} from "../src/contracts/subdomainregistrar/pricers/FixedPricer.sol";
import {ISubdomainPricer} from "../src/contracts/subdomainregistrar/pricers/ISubdomainPricer.sol";

/// @notice Deploys ForeverSubdomainRegistrar on Sepolia using the real ENS contracts.
///
/// Prerequisites:
///   - You must own a .eth name on Sepolia (e.g., boringtun.eth)
///
/// Usage:
///   export PRIVATE_KEY=0x...
///   export PARENT_LABEL=boringtun
///   forge script script/DeploySepolia.s.sol:DeploySepolia \
///     --rpc-url https://rpc.sepolia.org \
///     --broadcast --verify
contract DeploySepolia is Script {
    bytes32 constant ETH_NODE = 0x93cdeb708b7545dc668eb9280176169d1c33cfd8ed6f04690a0bcc88a93fc4ae;

    address constant ENS_REGISTRY = 0x00000000000C2E074eC69A0dFb2997BA6C7d2e1e;
    address constant NAME_WRAPPER = 0x0635513f179D50A207757E05759CbD106d7dFcE8;
    address constant BASE_REGISTRAR = 0x57f1887a8BF19b14fC0dF6Fd9B2acc9Af147eA85;
    address constant PUBLIC_RESOLVER = 0x8FADE66B79cC9f707aB26799354482EB93a5B7dD;

    function run() external {
        uint256 deployerKey = vm.envUint("PRIVATE_KEY");
        string memory parentLabel = vm.envString("PARENT_LABEL");
        address deployer = vm.addr(deployerKey);

        INameWrapper nameWrapper = INameWrapper(NAME_WRAPPER);
        IBaseRegistrar baseRegistrar = IBaseRegistrar(BASE_REGISTRAR);

        bytes32 parentLabelhash = keccak256(bytes(parentLabel));
        bytes32 parentNode = keccak256(abi.encodePacked(ETH_NODE, parentLabelhash));

        vm.startBroadcast(deployerKey);

        // Wrap the ENS name if not already wrapped
        if (!nameWrapper.isWrapped(parentNode)) {
            console.log("Parent name is NOT wrapped. Wrapping now...");
            baseRegistrar.setApprovalForAll(NAME_WRAPPER, true);
            nameWrapper.wrapETH2LD(
                parentLabel,
                deployer,
                uint16(CANNOT_UNWRAP),
                PUBLIC_RESOLVER
            );
            console.log("Parent name wrapped and locked (CANNOT_UNWRAP).");
        } else {
            console.log("Parent name is already wrapped.");
        }

        // Deploy ForeverSubdomainRegistrar
        ForeverSubdomainRegistrar registrar = new ForeverSubdomainRegistrar(NAME_WRAPPER);
        console.log("ForeverSubdomainRegistrar:", address(registrar));

        // Approve registrar as operator on NameWrapper
        nameWrapper.setApprovalForAll(address(registrar), true);

        // Deploy free FixedPricer and set up the domain
        FixedPricer pricer = new FixedPricer(0, address(0));
        registrar.setupDomain(parentNode, ISubdomainPricer(address(pricer)), deployer, true);

        vm.stopBroadcast();

        console.log("\n=== Deployment Summary (Sepolia) ===");
        console.log("ENSRegistry:                ", ENS_REGISTRY);
        console.log("NameWrapper:                ", NAME_WRAPPER);
        console.log("PublicResolver:             ", PUBLIC_RESOLVER);
        console.log("ForeverSubdomainRegistrar:  ", address(registrar));
        console.log("FixedPricer:                ", address(pricer));
        console.log("Parent Label:               ", parentLabel);
        console.log("Parent Node:                ");
        console.logBytes32(parentNode);
    }
}
