// SPDX-License-Identifier: MIT
pragma solidity ~0.8.17;

import "forge-std/Script.sol";
import {INameWrapper, CANNOT_UNWRAP} from "../src/contracts/wrapper/INameWrapper.sol";
import {IBaseRegistrar} from "../src/contracts/ethregistrar/IBaseRegistrar.sol";
import {ForeverSubdomainRegistrar} from "../src/contracts/subdomainregistrar/ForeverSubdomainRegistrar.sol";
import {FixedPricer} from "../src/contracts/subdomainregistrar/pricers/FixedPricer.sol";
import {ISubdomainPricer} from "../src/contracts/subdomainregistrar/pricers/ISubdomainPricer.sol";

/// @notice Deploys ForeverSubdomainRegistrar on any network with existing ENS contracts.
///         Automatically wraps the parent ENS name if it is not already wrapped.
///
/// Required env vars:
///   PRIVATE_KEY    - deployer private key (must be the ENS name owner)
///   PARENT_LABEL   - the .eth label (e.g. "boringtun" for boringtun.eth)
///
/// Optional env vars:
///   NAME_WRAPPER     - override NameWrapper address
///   BASE_REGISTRAR   - override BaseRegistrar address
///   PUBLIC_RESOLVER  - override PublicResolver address
///
/// Usage:
///   forge script script/Deploy.s.sol:Deploy --rpc-url $RPC_URL --broadcast
contract Deploy is Script {
    bytes32 constant ETH_NODE = 0x93cdeb708b7545dc668eb9280176169d1c33cfd8ed6f04690a0bcc88a93fc4ae;

    address constant DEFAULT_NAME_WRAPPER = 0x0635513f179D50A207757E05759CbD106d7dFcE8;
    address constant DEFAULT_BASE_REGISTRAR = 0x57f1887a8BF19b14fC0dF6Fd9B2acc9Af147eA85;
    address constant DEFAULT_PUBLIC_RESOLVER = 0x8FADE66B79cC9f707aB26799354482EB93a5B7dD;

    function run() external {
        uint256 deployerKey = vm.envUint("PRIVATE_KEY");
        string memory parentLabel = vm.envString("PARENT_LABEL");
        address deployer = vm.addr(deployerKey);

        address nameWrapperAddr = vm.envOr("NAME_WRAPPER", DEFAULT_NAME_WRAPPER);
        address baseRegistrarAddr = vm.envOr("BASE_REGISTRAR", DEFAULT_BASE_REGISTRAR);
        address resolverAddr = vm.envOr("PUBLIC_RESOLVER", DEFAULT_PUBLIC_RESOLVER);

        INameWrapper nameWrapper = INameWrapper(nameWrapperAddr);
        IBaseRegistrar baseRegistrar = IBaseRegistrar(baseRegistrarAddr);

        bytes32 parentLabelhash = keccak256(bytes(parentLabel));
        bytes32 parentNode = keccak256(abi.encodePacked(ETH_NODE, parentLabelhash));

        vm.startBroadcast(deployerKey);

        // --- Wrap the ENS name if not already wrapped ---
        if (!nameWrapper.isWrapped(parentNode)) {
            console.log("Parent name is NOT wrapped. Wrapping now...");
            baseRegistrar.setApprovalForAll(nameWrapperAddr, true);
            nameWrapper.wrapETH2LD(parentLabel, deployer, uint16(CANNOT_UNWRAP), resolverAddr);
            console.log("Parent name wrapped and locked (CANNOT_UNWRAP).");
        } else {
            console.log("Parent name is already wrapped.");
        }

        // --- Deploy ForeverSubdomainRegistrar ---
        ForeverSubdomainRegistrar registrar = new ForeverSubdomainRegistrar(nameWrapperAddr);
        console.log("ForeverSubdomainRegistrar:", address(registrar));

        // --- Approve registrar as operator on NameWrapper ---
        nameWrapper.setApprovalForAll(address(registrar), true);

        // --- Deploy free FixedPricer (0 fee, no token) and set up the domain ---
        FixedPricer pricer = new FixedPricer(0, address(0));
        registrar.setupDomain(parentNode, ISubdomainPricer(address(pricer)), deployer, true);
        console.log("Domain set up with free FixedPricer.");

        vm.stopBroadcast();

        console.log("\n=== Deployment Summary ===");
        console.log("NameWrapper:                ", nameWrapperAddr);
        console.log("BaseRegistrar:              ", baseRegistrarAddr);
        console.log("PublicResolver:             ", resolverAddr);
        console.log("ForeverSubdomainRegistrar:  ", address(registrar));
        console.log("FixedPricer:                ", address(pricer));
        console.log("Parent Label:               ", parentLabel);
        console.log("Parent Node:                ");
        console.logBytes32(parentNode);
    }
}
