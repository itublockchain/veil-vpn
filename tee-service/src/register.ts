import "dotenv/config";
import { ethers } from "ethers";
import { registerSubname, type EnsConfig, type VpnTextRecords } from "./utils/ens.js";
import { registerAttestation, type SapphireConfig } from "./utils/sapphire.js";

function env(key: string, fallback?: string): string {
  const val = process.env[key] ?? fallback;
  if (!val) throw new Error(`Missing env var: ${key}`);
  return val;
}

async function main() {
  console.log("=== TEE VPN Node Registration ===\n");

  // 1. Setup Sepolia wallet
  const privateKey = env("PRIVATE_KEY");
  const sepoliaProvider = new ethers.JsonRpcProvider(env("SEPOLIA_RPC_URL"));
  const sepoliaWallet = new ethers.Wallet(privateKey, sepoliaProvider);

  console.log(`[Init] Wallet address: ${sepoliaWallet.address}`);

  // 2. Config
  const label = env("LABEL", `node-${Date.now()}`);
  const publicIp = env("PUBLIC_IP", "127.0.0.1");
  const wsPort = env("WS_PORT", "8443");
  const httpPort = env("HTTP_PORT", "8080");
  const wgPublicKey = env("WG_PUBLIC_KEY");

  const ensConfig: EnsConfig = {
    registrarAddress: env("FOREVER_REGISTRAR"),
    resolverAddress: env("PUBLIC_RESOLVER", "0x8FADE66B79cC9f707aB26799354482EB93a5B7dD"),
    parentNode: env("PARENT_NODE"),
  };

  const sapphireConfig: SapphireConfig = {
    rpcUrl: env("SAPPHIRE_RPC_URL"),
    registryAddress: env("TEE_REGISTRY"),
  };

  const vpnRecords: VpnTextRecords = {
    publicKey: wgPublicKey,
    endpoint: `${publicIp}:${wsPort}`,
    wsEndpoint: `ws://${publicIp}:${wsPort}`,
    httpUrl: `http://${publicIp}:${httpPort}`,
  };

  // 3. Step 1 — Register ENS subname on Sepolia
  console.log("\n--- Step 1: ENS Registration (Sepolia) ---");
  const ensNode = await registerSubname(sepoliaWallet, ensConfig, label, vpnRecords);

  // 4. Step 2 — Register attestation on Sapphire
  //    Uses ROFL daemon socket if available (TEE), falls back to wallet (dev)
  console.log("\n--- Step 2: TEE Attestation (Sapphire) ---");
  try {
    await registerAttestation(sapphireConfig, ensNode, label, privateKey);
  } catch (err) {
    console.error(`[Sapphire] Attestation failed (ENS subname exists but unattested):`, err);
    console.log("[Sapphire] Clients will ignore this node until attestation succeeds.");
  }

  // 5. Summary
  console.log("\n=== Registration Complete ===");
  console.log(`Label:       ${label}`);
  console.log(`ENS Node:    ${ensNode}`);
  console.log(`Public Key:  ${wgPublicKey}`);
  console.log(`WS Endpoint: ws://${publicIp}:${wsPort}`);
  console.log(`HTTP API:    http://${publicIp}:${httpPort}`);
}

main().catch((err) => {
  console.error("Fatal:", err);
  process.exit(1);
});
