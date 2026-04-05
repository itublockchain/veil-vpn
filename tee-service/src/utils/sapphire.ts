import { ethers } from "ethers";
import * as sapphire from "@oasisprotocol/sapphire-paratime";
import { execSync } from "node:child_process";

const TEE_REGISTRY_ABI = [
  "function register(bytes32 ensNode, string label)",
  "function getAttestation(bytes32 ensNode) view returns (address attester, string label, uint256 timestamp)",
  "function getSubnames() view returns (bytes32[])",
  "function isAttested(bytes32 ensNode) view returns (bool)",
];

const ROFL_DAEMON_SOCKET = "/run/rofl-appd.sock";

export interface SapphireConfig {
  rpcUrl: string;
  registryAddress: string;
}

/**
 * Create a Sapphire-wrapped provider (for read-only calls).
 */
export function createSapphireProvider(rpcUrl: string): ethers.JsonRpcProvider {
  const provider = new ethers.JsonRpcProvider(rpcUrl);
  return sapphire.wrap(provider) as unknown as ethers.JsonRpcProvider;
}

/**
 * Register an attestation on Sapphire via the ROFL daemon socket.
 * The daemon signs and submits the tx with TEE-attested keys.
 * Falls back to regular wallet if daemon socket is not available (dev mode).
 */
export async function registerAttestation(
  config: SapphireConfig,
  ensNode: string,
  label: string,
  privateKey?: string
): Promise<void> {
  const iface = new ethers.Interface(TEE_REGISTRY_ABI);
  const calldata = iface.encodeFunctionData("register", [ensNode, label]);

  const daemonAvailable = await isDaemonAvailable();

  if (daemonAvailable) {
    console.log(`[Sapphire] Submitting via ROFL daemon socket`);
    await submitViaRoflDaemon(config.registryAddress, calldata);
  } else if (privateKey) {
    console.log(`[Sapphire] ROFL daemon not available, using wallet (dev mode)`);
    const provider = new ethers.JsonRpcProvider(config.rpcUrl);
    const signer = sapphire.wrap(new ethers.Wallet(privateKey, provider)) as unknown as ethers.Signer;
    const registry = new ethers.Contract(config.registryAddress, TEE_REGISTRY_ABI, signer);
    const tx = await registry.register(ensNode, label);
    console.log(`[Sapphire] Attestation tx: ${tx.hash}`);
    await tx.wait();
  } else {
    throw new Error("ROFL daemon not available and no PRIVATE_KEY for fallback");
  }

  console.log(`[Sapphire] Attestation confirmed`);
}

/**
 * Submit a transaction via the ROFL appd Unix socket.
 * POST to /rofl/v1/tx/sign-submit
 */
async function submitViaRoflDaemon(to: string, data: string): Promise<void> {
  const body = JSON.stringify({
    tx: {
      kind: "eth",
      data: {
        gas_limit: 300000,
        to,
        value: 0,
        data,
      },
    },
  });

  // Use curl to talk to the Unix socket (simplest cross-runtime approach)
  const result = execSync(
    `curl -s --fail --unix-socket ${ROFL_DAEMON_SOCKET} -X POST -H "Content-Type: application/json" -d '${body}' http://localhost/rofl/v1/tx/sign-submit`,
    { encoding: "utf-8", timeout: 30000 }
  );

  console.log(`[Sapphire] ROFL daemon response: ${result.trim()}`);
}

async function isDaemonAvailable(): Promise<boolean> {
  try {
    execSync(
      `curl -s --fail --unix-socket ${ROFL_DAEMON_SOCKET} http://localhost/rofl/v1/status`,
      { encoding: "utf-8", timeout: 5000 }
    );
    return true;
  } catch {
    return false;
  }
}

export interface AttestationInfo {
  ensNode: string;
  attester: string;
  label: string;
  timestamp: number;
}

/**
 * Get all attested subnames from the registry.
 */
export async function getAttestedNodes(
  config: SapphireConfig
): Promise<AttestationInfo[]> {
  const provider = createSapphireProvider(config.rpcUrl);
  const registry = new ethers.Contract(config.registryAddress, TEE_REGISTRY_ABI, provider);

  const nodes: string[] = await registry.getSubnames();
  const results: AttestationInfo[] = [];

  for (const ensNode of nodes) {
    const [attester, label, timestamp] = await registry.getAttestation(ensNode);
    results.push({
      ensNode,
      attester,
      label,
      timestamp: Number(timestamp),
    });
  }

  return results;
}

/**
 * Check if a specific ENS node is attested.
 */
export async function isAttested(
  config: SapphireConfig,
  ensNode: string
): Promise<boolean> {
  const provider = createSapphireProvider(config.rpcUrl);
  const registry = new ethers.Contract(config.registryAddress, TEE_REGISTRY_ABI, provider);
  return registry.isAttested(ensNode);
}
