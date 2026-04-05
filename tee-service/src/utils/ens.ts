import { ethers, type Signer } from "ethers";

const FOREVER_REGISTRAR_ABI = [
  "function register(bytes32 parentNode, string label, address newOwner, address resolver, uint16 fuses, bytes[] records) payable",
];

const RESOLVER_ABI = [
  "function setText(bytes32 node, string key, string value)",
];

export interface EnsConfig {
  registrarAddress: string;
  resolverAddress: string;
  parentNode: string;
}

export interface VpnTextRecords {
  publicKey: string;
  endpoint?: string;
  wsEndpoint?: string;
  httpUrl?: string;
}

/**
 * Register an ENS subname under the parent node and set VPN text records.
 * Returns the computed ENS node hash for the new subname.
 */
export async function registerSubname(
  signer: Signer,
  config: EnsConfig,
  label: string,
  records: VpnTextRecords
): Promise<string> {
  const signerAddr = await signer.getAddress();

  // Compute the subname node: keccak256(parentNode, keccak256(label))
  const labelHash = ethers.keccak256(ethers.toUtf8Bytes(label));
  const ensNode = ethers.keccak256(
    ethers.solidityPacked(["bytes32", "bytes32"], [config.parentNode, labelHash])
  );

  console.log(`[ENS] Registering subname: ${label}`);
  console.log(`[ENS] ENS node: ${ensNode}`);

  // Build setText records to pass during registration
  const resolverIface = new ethers.Interface(RESOLVER_ABI);
  const recordCalldata: string[] = [];

  // Always set vpn.publickey
  recordCalldata.push(
    resolverIface.encodeFunctionData("setText", [ensNode, "vpn.publickey", records.publicKey])
  );

  const registrar = new ethers.Contract(config.registrarAddress, FOREVER_REGISTRAR_ABI, signer);

  const tx = await registrar.register(
    config.parentNode,
    label,
    signerAddr,
    config.resolverAddress,
    0, // no extra fuses
    recordCalldata
  );
  console.log(`[ENS] Register tx: ${tx.hash}`);
  await tx.wait();
  console.log(`[ENS] Register confirmed`);

  // Set additional text records directly on the resolver
  const resolver = new ethers.Contract(config.resolverAddress, RESOLVER_ABI, signer);

  if (records.endpoint) {
    const tx2 = await resolver.setText(ensNode, "vpn.endpoint", records.endpoint);
    await tx2.wait();
    console.log(`[ENS] Set vpn.endpoint: ${records.endpoint}`);
  }

  if (records.wsEndpoint) {
    const tx3 = await resolver.setText(ensNode, "vpn.ws", records.wsEndpoint);
    await tx3.wait();
    console.log(`[ENS] Set vpn.ws: ${records.wsEndpoint}`);
  }

  if (records.httpUrl) {
    const tx4 = await resolver.setText(ensNode, "vpn.http", records.httpUrl);
    await tx4.wait();
    console.log(`[ENS] Set vpn.http: ${records.httpUrl}`);
  }

  return ensNode;
}

/**
 * Read VPN text records for a given ENS node.
 */
export async function readVpnRecords(
  provider: ethers.Provider,
  resolverAddress: string,
  ensNode: string
): Promise<VpnTextRecords & { [key: string]: string }> {
  const resolver = new ethers.Contract(
    resolverAddress,
    ["function text(bytes32 node, string key) view returns (string)"],
    provider
  );

  const [publicKey, endpoint, wsEndpoint, httpUrl] = await Promise.all([
    resolver.text(ensNode, "vpn.publickey"),
    resolver.text(ensNode, "vpn.endpoint"),
    resolver.text(ensNode, "vpn.ws"),
    resolver.text(ensNode, "vpn.http"),
  ]);

  return { publicKey, endpoint, wsEndpoint, httpUrl };
}
