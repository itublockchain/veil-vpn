import "dotenv/config";
import { ethers } from "ethers";
import { getAttestedNodes, type SapphireConfig } from "./utils/sapphire.js";
import { readVpnRecords } from "./utils/ens.js";

function env(key: string, fallback?: string): string {
  const val = process.env[key] ?? fallback;
  if (!val) throw new Error(`Missing env var: ${key}`);
  return val;
}

async function main() {
  console.log("=== TEE VPN Node Discovery ===\n");

  const sapphireConfig: SapphireConfig = {
    rpcUrl: env("SAPPHIRE_RPC_URL"),
    registryAddress: env("TEE_REGISTRY"),
  };

  const resolverAddress = env("PUBLIC_RESOLVER", "0x8FADE66B79cC9f707aB26799354482EB93a5B7dD");
  const sepoliaProvider = new ethers.JsonRpcProvider(env("SEPOLIA_RPC_URL"));

  // 1. Get all attested nodes from Sapphire
  console.log("[Discovery] Querying Sapphire for attested nodes...");
  const attestedNodes = await getAttestedNodes(sapphireConfig);
  console.log(`[Discovery] Found ${attestedNodes.length} attested node(s)\n`);

  if (attestedNodes.length === 0) {
    console.log("No attested VPN nodes found.");
    return;
  }

  // 2. For each attested node, read ENS text records from Sepolia
  const nodes = [];
  for (const node of attestedNodes) {
    const records = await readVpnRecords(sepoliaProvider, resolverAddress, node.ensNode);
    nodes.push({
      label: node.label,
      attester: node.attester,
      attestedAt: new Date(node.timestamp * 1000).toISOString(),
      publicKey: records.publicKey || "(not set)",
      endpoint: records.endpoint || "(not set)",
      wsEndpoint: records.wsEndpoint || "(not set)",
      httpUrl: records.httpUrl || "(not set)",
    });
  }

  // 3. Display
  console.log("=== Verified VPN Nodes ===\n");
  console.table(nodes);
}

main().catch((err) => {
  console.error("Fatal:", err);
  process.exit(1);
});
