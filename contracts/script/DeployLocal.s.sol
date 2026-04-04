// SPDX-License-Identifier: MIT
pragma solidity ~0.8.17;

import "forge-std/Script.sol";
import {ENSRegistry} from "../src/contracts/registry/ENSRegistry.sol";
import {BaseRegistrarImplementation} from "../src/contracts/ethregistrar/BaseRegistrarImplementation.sol";
import {ReverseRegistrar} from "../src/contracts/reverseRegistrar/ReverseRegistrar.sol";
import {NameWrapper} from "../src/contracts/wrapper/NameWrapper.sol";
import {INameWrapper, CANNOT_UNWRAP} from "../src/contracts/wrapper/INameWrapper.sol";
import {IMetadataService} from "../src/contracts/wrapper/IMetadataService.sol";
import {StaticMetadataService} from "../src/contracts/wrapper/StaticMetadataService.sol";
import {PublicResolver} from "../src/contracts/resolvers/PublicResolver.sol";
import {ForeverSubdomainRegistrar} from "../src/contracts/subdomainregistrar/ForeverSubdomainRegistrar.sol";
import {FixedPricer} from "../src/contracts/subdomainregistrar/pricers/FixedPricer.sol";
import {ISubdomainPricer} from "../src/contracts/subdomainregistrar/pricers/ISubdomainPricer.sol";

/// @notice Deploys the full ENS stack + ForeverSubdomainRegistrar on a local Anvil chain.
/// Usage:
///   anvil &
///   forge script script/DeployLocal.s.sol:DeployLocal --rpc-url http://127.0.0.1:8545 --broadcast
contract DeployLocal is Script {
    bytes32 constant ROOT_NODE = bytes32(0);
    bytes32 constant ETH_LABELHASH = keccak256("eth");
    bytes32 constant ETH_NODE = 0x93cdeb708b7545dc668eb9280176169d1c33cfd8ed6f04690a0bcc88a93fc4ae;
    bytes32 constant REVERSE_LABELHASH = keccak256("reverse");
    bytes32 constant ADDR_LABELHASH = keccak256("addr");

    function run() external {
        uint256 deployerKey = vm.envOr("PRIVATE_KEY", uint256(0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80));
        string memory parentLabel = vm.envOr("PARENT_LABEL", string("boringtun"));
        address deployer = vm.addr(deployerKey);

        vm.startBroadcast(deployerKey);

        // Deploy ENS core
        (ENSRegistry ens, NameWrapper nameWrapper, PublicResolver publicResolver) = _deployENS(deployer);

        // Register and wrap parent domain
        bytes32 parentNode = _setupParent(ens, nameWrapper, parentLabel, deployer, address(publicResolver));

        // Deploy subdomain registrar
        ForeverSubdomainRegistrar registrar = new ForeverSubdomainRegistrar(address(nameWrapper));
        nameWrapper.setApprovalForAll(address(registrar), true);

        FixedPricer pricer = new FixedPricer(0, address(0));
        registrar.setupDomain(parentNode, ISubdomainPricer(address(pricer)), deployer, true);

        vm.stopBroadcast();

        console.log("\n=== Deployment Summary (Local) ===");
        console.log("ENSRegistry:                ", address(ens));
        console.log("NameWrapper:                ", address(nameWrapper));
        console.log("PublicResolver:             ", address(publicResolver));
        console.log("ForeverSubdomainRegistrar:  ", address(registrar));
        console.log("FixedPricer:                ", address(pricer));
        console.log("Parent Node:                ");
        console.logBytes32(parentNode);
    }

    function _deployENS(address deployer) internal returns (ENSRegistry, NameWrapper, PublicResolver) {
        ENSRegistry ens = new ENSRegistry();

        // Reverse registrar
        ens.setSubnodeOwner(ROOT_NODE, REVERSE_LABELHASH, deployer);
        bytes32 reverseNode = keccak256(abi.encodePacked(ROOT_NODE, REVERSE_LABELHASH));
        ReverseRegistrar reverseRegistrar = new ReverseRegistrar(ens);
        ens.setSubnodeOwner(reverseNode, ADDR_LABELHASH, address(reverseRegistrar));

        // .eth TLD + BaseRegistrar
        ens.setSubnodeOwner(ROOT_NODE, ETH_LABELHASH, deployer);
        BaseRegistrarImplementation baseRegistrar = new BaseRegistrarImplementation(ens, ETH_NODE);
        ens.setSubnodeOwner(ROOT_NODE, ETH_LABELHASH, address(baseRegistrar));

        // NameWrapper
        StaticMetadataService metadataService = new StaticMetadataService("https://ens.domains");
        NameWrapper nameWrapper = new NameWrapper(ens, baseRegistrar, IMetadataService(address(metadataService)));
        baseRegistrar.addController(address(nameWrapper));
        baseRegistrar.addController(deployer);

        // PublicResolver
        PublicResolver publicResolver = new PublicResolver(
            ens, INameWrapper(address(nameWrapper)), deployer, address(reverseRegistrar)
        );

        return (ens, nameWrapper, publicResolver);
    }

    function _setupParent(
        ENSRegistry ens,
        NameWrapper nameWrapper,
        string memory parentLabel,
        address deployer,
        address resolver
    ) internal returns (bytes32) {
        bytes32 parentLabelhash = keccak256(bytes(parentLabel));
        BaseRegistrarImplementation baseRegistrar = BaseRegistrarImplementation(address(nameWrapper.registrar()));

        baseRegistrar.register(uint256(parentLabelhash), deployer, 365 days);

        bytes32 parentNode = keccak256(abi.encodePacked(ETH_NODE, parentLabelhash));
        ens.setResolver(parentNode, resolver);

        baseRegistrar.setApprovalForAll(address(nameWrapper), true);
        nameWrapper.wrapETH2LD(parentLabel, deployer, uint16(CANNOT_UNWRAP), resolver);

        return parentNode;
    }
}
