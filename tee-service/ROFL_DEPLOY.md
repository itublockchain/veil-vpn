# ROFL Deployment Guide

## Prerequisites

- Oasis CLI installed
- ~150 TEST tokens on Sapphire testnet
- ENS ForeverSubdomainRegistrar deployed on Sepolia
- `.env` file configured (copy from `.env.example`)

## Steps

### 1. Install Oasis CLI

```bash
brew install oasis
```

### 2. Create wallet and fund it

```bash
oasis wallet create my_account
# Get TEST tokens from https://faucet.testnet.oasis.io/
```

### 3. Register ROFL app on Sapphire testnet

```bash
cd tee-service
oasis rofl create --network testnet --account my_account
```

Copy the output `app_id` (bytes21).

### 4. Update rofl.yaml

Set `deployments.testnet.app_id` in `rofl.yaml` to the app ID from step 3.

### 5. Deploy TEEAttestationRegistry

```bash
cd ../contracts

ROFL_APP_ID=0x<your-app-id> \
PRIVATE_KEY=0x... \
forge script script/DeploySapphire.s.sol:DeploySapphire \
  --rpc-url https://testnet.sapphire.oasis.io \
  --broadcast
```

Copy the deployed `TEE_REGISTRY` address.

### 6. Configure .env

```bash
cd ../tee-service
cp .env.example .env
# Fill in:
#   PRIVATE_KEY, SEPOLIA_RPC_URL, FOREVER_REGISTRAR, PARENT_NODE
#   SAPPHIRE_RPC_URL=https://testnet.sapphire.oasis.io
#   TEE_REGISTRY=<address from step 5>
#   WG_PUBLIC_KEY, PUBLIC_IP, LABEL
```

### 7. Encrypt secrets into ROFL manifest

```bash
oasis rofl secret import .env
```

### 8. Build ORC bundle

```bash
oasis rofl build
```

### 9. Push config to chain

```bash
oasis rofl update
```

### 10. Deploy to ROFL marketplace

```bash
oasis rofl deploy
```

### 11. Verify

```bash
# Check app status
oasis rofl show

# Check machine status
oasis rofl machine show

# View logs
oasis rofl machine logs
```

## Dev mode (without TEE)

To test locally without ROFL:

```bash
# Terminal 1: Start boringtun
BT_PAYMENT_SERVER=1 BT_HTTP_BIND=0.0.0.0:8080 BT_WS_BIND=0.0.0.0:8443 \
  WG_LOG_LEVEL=info cargo run --features payment -- tun0 -f

# Terminal 2: Run registration (falls back to wallet signing)
cd tee-service
npm install
npx tsx src/register.ts

# Terminal 3: Discover nodes
npx tsx src/discover.ts
```

In dev mode, Sapphire attestation will use the wallet directly (no ROFL verification). The contract's `roflEnsureAuthorizedOrigin` will reject this on testnet/mainnet — only works from a real ROFL instance.
