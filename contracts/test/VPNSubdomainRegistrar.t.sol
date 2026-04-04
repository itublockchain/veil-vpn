// SPDX-License-Identifier: MIT
pragma solidity ~0.8.17;

import "forge-std/Test.sol";
import {ENSRegistry} from "../src/contracts/registry/ENSRegistry.sol";
import {BaseRegistrarImplementation} from "../src/contracts/ethregistrar/BaseRegistrarImplementation.sol";
import {ReverseRegistrar} from "../src/contracts/reverseRegistrar/ReverseRegistrar.sol";
import {NameWrapper} from "../src/contracts/wrapper/NameWrapper.sol";
import {INameWrapper, CANNOT_UNWRAP, PARENT_CANNOT_CONTROL} from "../src/contracts/wrapper/INameWrapper.sol";
import {IMetadataService} from "../src/contracts/wrapper/IMetadataService.sol";
import {StaticMetadataService} from "../src/contracts/wrapper/StaticMetadataService.sol";
import {PublicResolver} from "../src/contracts/resolvers/PublicResolver.sol";
import {ForeverSubdomainRegistrar} from "../src/contracts/subdomainregistrar/ForeverSubdomainRegistrar.sol";
import {FixedPricer} from "../src/contracts/subdomainregistrar/pricers/FixedPricer.sol";
import {ISubdomainPricer} from "../src/contracts/subdomainregistrar/pricers/ISubdomainPricer.sol";
import {Unavailable} from "../src/contracts/subdomainregistrar/BaseSubdomainRegistrar.sol";

interface ITextResolver {
    function text(bytes32 node, string calldata key) external view returns (string memory);
}

contract VPNSubdomainTest is Test {
    // Required for receiving ERC1155 tokens from NameWrapper
    function onERC1155Received(address, address, uint256, uint256, bytes calldata) external pure returns (bytes4) {
        return this.onERC1155Received.selector;
    }
    function onERC1155BatchReceived(address, address, uint256[] calldata, uint256[] calldata, bytes calldata) external pure returns (bytes4) {
        return this.onERC1155BatchReceived.selector;
    }
    function supportsInterface(bytes4) external pure returns (bool) { return true; }

    ENSRegistry ens;
    BaseRegistrarImplementation baseRegistrar;
    NameWrapper nameWrapper;
    PublicResolver publicResolver;
    ForeverSubdomainRegistrar registrar;

    bytes32 constant ROOT_NODE = bytes32(0);
    bytes32 constant ETH_NODE = 0x93cdeb708b7545dc668eb9280176169d1c33cfd8ed6f04690a0bcc88a93fc4ae;

    address deployer = address(this);
    address alice = address(0xA11CE);
    address bob = address(0xB0B);

    bytes32 parentNode;

    function setUp() public {
        vm.warp(365 days);

        // 1. ENS Registry
        ens = new ENSRegistry();

        // 2. Reverse registrar namespace
        ens.setSubnodeOwner(ROOT_NODE, keccak256("reverse"), deployer);
        bytes32 reverseNode = keccak256(abi.encodePacked(ROOT_NODE, keccak256("reverse")));
        ReverseRegistrar reverseRegistrar = new ReverseRegistrar(ens);
        ens.setSubnodeOwner(reverseNode, keccak256("addr"), address(reverseRegistrar));

        // 3. .eth TLD
        ens.setSubnodeOwner(ROOT_NODE, keccak256("eth"), deployer);

        // 4. BaseRegistrar
        baseRegistrar = new BaseRegistrarImplementation(ens, ETH_NODE);
        ens.setSubnodeOwner(ROOT_NODE, keccak256("eth"), address(baseRegistrar));

        // 5. NameWrapper
        StaticMetadataService metadataService = new StaticMetadataService("https://ens.domains");
        nameWrapper = new NameWrapper(ens, baseRegistrar, IMetadataService(address(metadataService)));
        baseRegistrar.addController(address(nameWrapper));
        baseRegistrar.addController(deployer);

        // 6. PublicResolver
        publicResolver = new PublicResolver(
            ens,
            INameWrapper(address(nameWrapper)),
            deployer,
            address(reverseRegistrar)
        );

        // 7. Register and wrap parent domain "boringtun.eth"
        bytes32 parentLabelhash = keccak256("boringtun");
        baseRegistrar.register(uint256(parentLabelhash), deployer, 365 days);

        parentNode = keccak256(abi.encodePacked(ETH_NODE, parentLabelhash));
        ens.setResolver(parentNode, address(publicResolver));

        baseRegistrar.setApprovalForAll(address(nameWrapper), true);
        nameWrapper.wrapETH2LD("boringtun", deployer, uint16(CANNOT_UNWRAP), address(publicResolver));

        // 8. Deploy ForeverSubdomainRegistrar
        registrar = new ForeverSubdomainRegistrar(address(nameWrapper));

        // 9. Approve registrar as operator on NameWrapper
        nameWrapper.setApprovalForAll(address(registrar), true);

        // 10. Set up domain with free pricer
        FixedPricer pricer = new FixedPricer(0, address(0));
        registrar.setupDomain(parentNode, ISubdomainPricer(address(pricer)), deployer, true);
    }

    function _makeRecords(bytes32 node, string memory publicKey) internal pure returns (bytes[] memory) {
        bytes[] memory records = new bytes[](1);
        records[0] = abi.encodeWithSelector(
            ITextResolver.text.selector, // selector for setText is different, use raw
            node,
            "vpn.publickey",
            publicKey
        );
        return records;
    }

    function _makeSetTextRecord(bytes32 node, string memory key, string memory value) internal pure returns (bytes memory) {
        return abi.encodeWithSignature("setText(bytes32,string,string)", node, key, value);
    }

    function _buildRecords(
        bytes32 node,
        string memory publicKey,
        string memory httpUrl,
        string memory url,
        string memory metadata
    ) internal pure returns (bytes[] memory) {
        uint256 count = 1;
        if (bytes(httpUrl).length > 0) count++;
        if (bytes(url).length > 0) count++;
        if (bytes(metadata).length > 0) count++;

        bytes[] memory records = new bytes[](count);
        uint256 idx = 0;

        records[idx++] = _makeSetTextRecord(node, "vpn.publickey", publicKey);
        if (bytes(httpUrl).length > 0) {
            records[idx++] = _makeSetTextRecord(node, "vpn.http", httpUrl);
        }
        if (bytes(url).length > 0) {
            records[idx++] = _makeSetTextRecord(node, "vpn.url", url);
        }
        if (bytes(metadata).length > 0) {
            records[idx++] = _makeSetTextRecord(node, "vpn.metadata", metadata);
        }

        return records;
    }

    function test_register() public {
        bytes32 node = keccak256(abi.encodePacked(parentNode, keccak256("alice")));
        bytes[] memory records = _buildRecords(
            node,
            "YWxpY2VwdWJrZXk=",
            "https://alice.vpn.example.com",
            "https://alice.example.com",
            '{"version":"1.0"}'
        );

        vm.prank(alice);
        registrar.register(parentNode, "alice", alice, address(publicResolver), 0, records);

        // Check text records
        assertEq(ITextResolver(address(publicResolver)).text(node, "vpn.publickey"), "YWxpY2VwdWJrZXk=");
        assertEq(ITextResolver(address(publicResolver)).text(node, "vpn.http"), "https://alice.vpn.example.com");
        assertEq(ITextResolver(address(publicResolver)).text(node, "vpn.url"), "https://alice.example.com");
        assertEq(ITextResolver(address(publicResolver)).text(node, "vpn.metadata"), '{"version":"1.0"}');

        // Check ownership
        assertEq(nameWrapper.ownerOf(uint256(node)), alice);
    }

    function test_registerTwoUsers() public {
        bytes32 aliceNode = keccak256(abi.encodePacked(parentNode, keccak256("alice")));
        bytes32 bobNode = keccak256(abi.encodePacked(parentNode, keccak256("bob")));

        bytes[] memory aliceRecords = _buildRecords(aliceNode, "alicePK", "https://alice.vpn", "", "");
        bytes[] memory bobRecords = _buildRecords(bobNode, "bobPK", "https://bob.vpn", "", "");

        vm.prank(alice);
        registrar.register(parentNode, "alice", alice, address(publicResolver), 0, aliceRecords);

        vm.prank(bob);
        registrar.register(parentNode, "bob", bob, address(publicResolver), 0, bobRecords);

        assertEq(ITextResolver(address(publicResolver)).text(aliceNode, "vpn.publickey"), "alicePK");
        assertEq(ITextResolver(address(publicResolver)).text(bobNode, "vpn.publickey"), "bobPK");
        assertEq(nameWrapper.ownerOf(uint256(aliceNode)), alice);
        assertEq(nameWrapper.ownerOf(uint256(bobNode)), bob);
    }

    function test_revertDuplicateRegistration() public {
        bytes32 node = keccak256(abi.encodePacked(parentNode, keccak256("alice")));
        bytes[] memory records = _buildRecords(node, "alicePK", "", "", "");

        vm.prank(alice);
        registrar.register(parentNode, "alice", alice, address(publicResolver), 0, records);

        bytes[] memory records2 = _buildRecords(node, "bobPK", "", "", "");
        vm.prank(bob);
        vm.expectRevert(Unavailable.selector);
        registrar.register(parentNode, "alice", bob, address(publicResolver), 0, records2);
    }

    function test_available() public {
        bytes32 node = keccak256(abi.encodePacked(parentNode, keccak256("alice")));
        assertTrue(registrar.available(node));

        bytes[] memory records = _buildRecords(node, "alicePK", "", "", "");
        vm.prank(alice);
        registrar.register(parentNode, "alice", alice, address(publicResolver), 0, records);

        assertFalse(registrar.available(node));

        bytes32 bobNode = keccak256(abi.encodePacked(parentNode, keccak256("bob")));
        assertTrue(registrar.available(bobNode));
    }

    function test_ownerCanUpdateTextRecords() public {
        bytes32 node = keccak256(abi.encodePacked(parentNode, keccak256("alice")));
        bytes[] memory records = _buildRecords(node, "oldPK", "", "", "");

        vm.prank(alice);
        registrar.register(parentNode, "alice", alice, address(publicResolver), 0, records);

        // Alice owns the subdomain, so she can update text records directly
        vm.prank(alice);
        publicResolver.setText(node, "vpn.publickey", "newPK");

        assertEq(ITextResolver(address(publicResolver)).text(node, "vpn.publickey"), "newPK");
    }

    function test_nonOwnerCannotUpdateTextRecords() public {
        bytes32 node = keccak256(abi.encodePacked(parentNode, keccak256("alice")));
        bytes[] memory records = _buildRecords(node, "alicePK", "", "", "");

        vm.prank(alice);
        registrar.register(parentNode, "alice", alice, address(publicResolver), 0, records);

        vm.prank(bob);
        vm.expectRevert();
        publicResolver.setText(node, "vpn.publickey", "hackedPK");

        assertEq(ITextResolver(address(publicResolver)).text(node, "vpn.publickey"), "alicePK");
    }

    function test_registerWithNoRecords() public {
        bytes[] memory records = new bytes[](0);

        vm.prank(alice);
        registrar.register(parentNode, "minimal", alice, address(publicResolver), 0, records);

        bytes32 node = keccak256(abi.encodePacked(parentNode, keccak256("minimal")));
        assertEq(nameWrapper.ownerOf(uint256(node)), alice);
    }
}
