<img width="1500" height="500" alt="veil_banner" src="https://github.com/user-attachments/assets/03cb256b-d8ae-447e-9a9e-673cb4e30e0d" />

# 🛡️ Veil VPN

**Don't trust. Verify.**

Veil is the first VPN protocol with **verifiable privacy**: pay-as-you-go, fully open source, and cryptographically proven to keep no logs.

---

## 🧾 What is "Veil"?

Veil is a **Verifiable Encrypted Internet Layer**: a trustless VPN protocol where privacy isn't promised, it's proven on-chain.

Every VPN claims "no logs." None of them prove it. The server side is a black box. Even open-source VPNs can't prove they're actually running the published code. This makes the entire privacy ecosystem fragile: every privacy tool is built on top of a potentially logging tunnel.

Whether you're:

- Tired of trusting VPN marketing claims
- Looking for verifiable, cryptographic privacy guarantees
- Wanting to pay only for what you use: no subscriptions, no accounts

→ Veil runs your VPN inside hardware-sealed enclaves, attests the exact running code on-chain, and lets anyone verify it. One line changes, the attestation breaks.

No trust required. No logs possible. Just internet privacy the way it should be: **verifiable and permissionless.**

---

## ⚙️ How It Works

### 🔐 Trustless VPN with TEE Attestation

We forked WireGuard and extended it to be crypto-native:

- **Zero-state server**: runs in Rust inside Oasis ROFL's TEE, holding no logs, no IPs, nothing on disk
- **Deterministic code hashing**: the TEE measures the exact code in the enclave and signs it
- **On-chain attestation**: ROFL publishes the attestation through a Solidity contract on Sapphire

Open source lets you read the code. Attestation proves it's running.

### 💸 Pay-Per-Use with Circle Nanopayments

No subscriptions. No accounts. No identity required:

- **Off-chain payment authorizations**: users sign via x402
- **Signature verification**: the VPN server verifies before opening the tunnel
- **Zero friction**: no transaction per session, no sign-up, just connect and pay

### 🌐 Permissionless Discovery with ENS

Every attested VPN node registers under `veil.eth` with on-chain metadata:

- **Endpoint, WireGuard public key, and payment address**: packed into ENS text records
- **Client resolution**: resolve the subdomain and connect directly
- **No centralized server list**: fully on-chain discovery, anyone can deploy a node

Only attested nodes get listed. Pass attestation, register your subdomain, start serving.

### 🧬 Human-Only Servers with World ID

Dedicated IPs where every connected user is verified as human:

- ✅ Zero-knowledge proof of personhood via World ID
- ✅ Services whitelist these IPs: eliminating captchas and rate limits
- ✅ No personal data exposed: just proof that you're human

---

Privacy shouldn't be a promise. It should be a **proof.**

**Built for ETHGlobal Cannes Hackathon 2026**
