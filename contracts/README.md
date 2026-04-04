# BoringTun VPN - ENS-based Peer Discovery

Decentralized VPN peer discovery using ENS (Ethereum Name Service) subdomains and text records.

## Architecture

```
                    boringtun.eth (parent domain, locked)
                         |
           +-------------+-------------+
           |             |             |
     alice.boringtun.eth  bob.boringtun.eth  ...
           |             |
     Text Records:  Text Records:
     vpn.publickey  vpn.publickey
     vpn.http       vpn.http
     vpn.url        vpn.url
     vpn.metadata   vpn.metadata
```

**How it works:**

1. An admin owns a parent ENS domain (e.g., `boringtun.eth`) and deploys `VPNSubdomainRegistrar`
2. Any VPN peer calls `register("alice", publicKey, httpUrl, url, metadata)` to create `alice.boringtun.eth`
3. The subdomain is **unruggable** - `PARENT_CANNOT_CONTROL` fuse is burned, so the admin can never revoke it
4. VPN clients query ENS text records to discover peer connection info
5. Subdomain owners can update their records directly on the PublicResolver

## Text Record Schema

| Key | Description | Example |
|-----|-------------|---------|
| `vpn.publickey` | WireGuard public key (base64) | `aGVsbG8gd29ybGQ=` |
| `vpn.http` | HTTP API endpoint for the VPN node | `https://node1.vpn.example.com` |
| `vpn.url` | General URL for the VPN node | `https://example.com/node1` |
| `vpn.metadata` | Arbitrary metadata (JSON recommended) | `{"version":"1.0","region":"eu"}` |

## Contracts

| Contract | Description |
|----------|-------------|
| `VPNSubdomainRegistrar` | Main contract - permissionless subdomain registration with VPN text records |
| ENS contracts (`src/contracts/`) | Core ENS stack from [ens-contracts](https://github.com/ensdomains/ens-contracts) |

## Prerequisites

- [Foundry](https://book.getfoundry.sh/getting-started/installation)
- For Sepolia: a funded wallet + an owned .eth domain

## Build & Test

```shell
forge build
forge test -vvv
```

## Deploy: Local Testnet (Anvil)

Deploys the **full ENS stack** (ENSRegistry, BaseRegistrar, NameWrapper, PublicResolver) plus VPNSubdomainRegistrar.

```shell
# Start local chain
anvil &

# Deploy (uses anvil's first account by default)
forge script script/DeployLocal.s.sol:DeployLocal \
  --rpc-url http://127.0.0.1:8545 \
  --broadcast

# Optional: customize parent domain label
PARENT_LABEL=myvpn forge script script/DeployLocal.s.sol:DeployLocal \
  --rpc-url http://127.0.0.1:8545 \
  --broadcast
```

### Test Registration (Local)

After deploying, use `cast` to register a subdomain:

```shell
# Get deployed addresses from the script output
VPN_REGISTRAR=<VPNSubdomainRegistrar address>

# Register as a VPN peer
cast send $VPN_REGISTRAR \
  "register(string,string,string,string,string)" \
  "alice" "YWxpY2VQdWJLZXk=" "https://alice.vpn.local" "https://alice.local" '{"region":"local"}' \
  --rpc-url http://127.0.0.1:8545 \
  --private-key 0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d

# Read the text records
RESOLVER=<PublicResolver address>
# First get the node hash
NODE=$(cast call $VPN_REGISTRAR "getSubdomainNode(string)(bytes32)" "alice" --rpc-url http://127.0.0.1:8545)

cast call $RESOLVER "text(bytes32,string)(string)" $NODE "vpn.publickey" --rpc-url http://127.0.0.1:8545
cast call $RESOLVER "text(bytes32,string)(string)" $NODE "vpn.http" --rpc-url http://127.0.0.1:8545
cast call $RESOLVER "text(bytes32,string)(string)" $NODE "vpn.url" --rpc-url http://127.0.0.1:8545
cast call $RESOLVER "text(bytes32,string)(string)" $NODE "vpn.metadata" --rpc-url http://127.0.0.1:8545
```

## Deploy: Sepolia Testnet

Uses the **real ENS contracts** on Sepolia - only deploys VPNSubdomainRegistrar.

| Contract | Sepolia Address |
|----------|-----------------|
| ENSRegistry | `0x00000000000C2E074eC69A0dFb2997BA6C7d2e1e` |
| NameWrapper | `0x0635513f179D50A207757E05759CbD106d7dFcE8` |
| PublicResolver | `0x8FADE66B79cC9f707aB26799354482EB93a5B7dD` |

### Prerequisites for Sepolia

1. **Own a .eth domain on Sepolia** - register at [sepolia.app.ens.domains](https://sepolia.app.ens.domains)
2. **Wrap and lock it** - the parent domain must have `CANNOT_UNWRAP` fuse burned (this is required for unruggable subdomains)
3. **Compute the namehash** of your domain:

```shell
# Install ensutils or compute manually
# namehash("boringtun.eth") example:
cast namehash "boringtun.eth"
```

### Deploy

```shell
export PRIVATE_KEY=0x...
export PARENT_NODE=$(cast namehash "yourdomain.eth")

forge script script/DeploySepolia.s.sol:DeploySepolia \
  --rpc-url https://rpc.sepolia.org \
  --broadcast \
  --verify
```

## Updating VPN Config

After registration, subdomain owners can update their text records **directly on the PublicResolver** (they own the subdomain via NameWrapper):

```shell
RESOLVER=<PublicResolver address>
NODE=<your subdomain node hash>

cast send $RESOLVER \
  "setText(bytes32,string,string)" \
  $NODE "vpn.publickey" "newPublicKeyBase64" \
  --rpc-url <rpc-url> \
  --private-key <your-private-key>
```

## VPN Client Integration

A VPN client discovers peers by querying ENS:

```
1. Client knows the parent domain (e.g., boringtun.eth)
2. Client queries resolver for alice.boringtun.eth text records:
   - vpn.publickey → WireGuard public key
   - vpn.http      → HTTP API endpoint
   - vpn.url       → Node URL
   - vpn.metadata  → Additional config
3. Client establishes WireGuard tunnel using discovered info
```

You can query ENS from any language using libraries like:
- **ethers.js**: `provider.getResolver("alice.boringtun.eth").then(r => r.getText("vpn.publickey"))`
- **viem**: `getEnsText({ name: "alice.boringtun.eth", key: "vpn.publickey" })`
- **Go**: `go-ens` package
- **Rust**: `ethers-rs` or `alloy`

## Project Structure

```
contracts/
├── src/
│   ├── VPNSubdomainRegistrar.sol   # Main VPN registrar contract
│   └── contracts/                   # ENS core contracts
│       ├── registry/                # ENSRegistry
│       ├── wrapper/                 # NameWrapper + fuses
│       ├── resolvers/               # PublicResolver + profiles (text, addr, etc.)
│       ├── ethregistrar/            # BaseRegistrar for .eth
│       └── reverseRegistrar/        # Reverse resolution
├── test/
│   └── VPNSubdomainRegistrar.t.sol  # Tests
├── script/
│   ├── DeployLocal.s.sol            # Local testnet (full ENS stack)
│   └── DeploySepolia.s.sol          # Sepolia (real ENS, only registrar)
└── foundry.toml
```
