# World ID Integration - Technical Implementation Plan

## Overview

Human-only VPN nodes. World ID Orb verification ile insanlığını kanıtlayan kullanıcılar temiz IP'ler üzerinden internete çıkar.

## Current Codebase Analysis

### Server: Register Endpoint

**File**: `protocol/boringtun/src/device/http_api.rs`

Current request:
```json
POST /v1/register
{ "public_key": "<base64_wg_pubkey>" }
```

Current response:
```json
{
  "status": "ok",
  "server_public_key": "<base64>",
  "endpoint": "<ip>:<port>",
  "assigned_ip": "<ip>/32",
  "chain_id": 5042002,
  "gateway_wallet": "0x...",
  "usdc_contract": "0x...",
  "amount_per_quota": 10000,
  "quota_bytes": 10485760
}
```

Key code locations:
- `handle_registration()` → line 228: Validates pubkey, allocates IP, calls UAPI
- `uapi_set_peer()` → line 177: Unix socket to WireGuard for peer add
- `RegistrationInner` → line 38: In-memory peer tracking (HashSet, HashMap)
- No authentication exists - any valid 32-byte key is accepted

### Client: Connect Flow

**File**: `client/src-tauri/src/vpn.rs`

1. `load_or_create_key()` → persistent x25519 key from `~/.veilvpn/client_key`
2. `derive_evm_address()` → HKDF-SHA256 → secp256k1 → keccak256 → wallet address
3. `POST /v1/register` with `{ "public_key": pub_b64 }`
4. Spawn boringtun-cli, configure WireGuard, setup routing

`RegisterResponse` struct (line 69):
```rust
struct RegisterResponse {
    server_public_key: String,
    endpoint: String,
    assigned_ip: String,
}
```

`ConnectedInfo` struct (line 37):
```rust
pub struct ConnectedInfo {
    pub assigned_ip: String,
    pub server_endpoint: String,
    pub wallet_address: String,
    pub gateway_balance: String,
}
```

---

## World ID API Specs

### Cloud Verification Endpoint

```
POST https://developer.world.org/api/v4/verify/{rp_id}
Content-Type: application/json
No authentication required (app identified by rp_id in URL)
```

Request body (v3.0 legacy - production ready):
```json
{
  "protocol_version": "3.0",
  "nonce": "uuid-from-rp-signature",
  "action": "veil-vpn-connect",
  "responses": [{
    "identifier": "orb",
    "signal_hash": "0x00...",
    "merkle_root": "0x...",
    "nullifier": "0x...",
    "proof": "0x...",
    "max_age": 86400
  }]
}
```

Success response:
```json
{
  "success": true,
  "action": "veil-vpn-connect",
  "nullifier": "0x...",
  "results": [{
    "identifier": "orb",
    "success": true,
    "nullifier": "0x..."
  }]
}
```

### signal_hash Computation

```
hash_to_field(input_bytes):
  h = keccak256(input_bytes)
  n = uint256(h) >> 8
  return n as 32-byte big-endian
```

Result always starts with 0x00. Empty string → `0x00c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a4`

### RP Signature (rp_context)

Server-side only. Uses signing_key from Developer Portal.

```
sign_request(signing_key, action, ttl=300):
  nonce = hash_to_field(random_32_bytes)
  created_at = now_unix
  expires_at = created_at + ttl
  msg = 0x01 || nonce(32) || created_at(8 BE) || expires_at(8 BE) || hash_to_field(action)(32)
  prefixed = "\x19Ethereum Signed Message:\n81" || msg
  digest = keccak256(prefixed)
  sig = secp256k1_sign_recoverable(signing_key, digest)
  return { sig: r||s||v, nonce, created_at, expires_at }
```

### IDKit in Tauri

Works in Tauri webview. Modal renders QR code in-DOM (no popup). User scans with World App.

```bash
npm install @worldcoin/idkit
```

---

## Implementation Plan

### Phase 1: Server - Accept World ID Proof in Register

**File**: `protocol/boringtun/src/device/http_api.rs`

#### 1.1 New Config

```rust
// Add to top of file
const WORLD_VERIFY_URL: &str = "https://developer.world.org/api/v4/verify";
```

Server env vars:
```bash
BT_HUMAN_ONLY=1              # Enable human-only mode
BT_WORLD_APP_ID=app_xxxxx    # From World ID Developer Portal
BT_WORLD_ACTION=veil-vpn-connect
```

#### 1.2 Extend Register Request

Current: `{ "public_key": "..." }`

New:
```json
{
  "public_key": "...",
  "world_proof": {                    // Required when BT_HUMAN_ONLY=1
    "merkle_root": "0x...",
    "nullifier_hash": "0x...",
    "proof": "0x...",
    "signal_hash": "0x..."
  }
}
```

signal = WireGuard public key (base64). Client computes signal_hash = hash_to_field(pubkey_b64_bytes).

#### 1.3 Modify handle_registration()

```rust
fn handle_registration(state: &RegistrationState, body: &str) -> (u16, String) {
    let json: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return (400, r#"{"error":"invalid JSON"}"#.into()),
    };

    let pubkey_b64 = match json["public_key"].as_str() {
        Some(s) => s,
        None => return (400, r#"{"error":"missing public_key"}"#.into()),
    };

    // ... existing pubkey validation ...

    // Human-only check
    if state.human_only {
        let world_proof = match json.get("world_proof") {
            Some(p) => p,
            None => return (400, r#"{"error":"world_proof required for human-only nodes"}"#.into()),
        };

        // 1. Verify with World API
        match verify_world_proof(state, world_proof, pubkey_b64) {
            Ok(nullifier) => {
                // 2. Check nullifier uniqueness
                let inner = state.inner.lock().unwrap();
                if inner.used_nullifiers.contains(&nullifier) {
                    return (403, r#"{"error":"already verified (one human = one connection)"}"#.into());
                }
                drop(inner);
                // Store nullifier after peer creation (below)
            }
            Err(e) => return (400, format!(r#"{{"error":"verification failed: {e}"}}"#)),
        }
    }

    // ... existing registration flow continues ...
    // After successful peer creation, store nullifier:
    // inner.used_nullifiers.insert(nullifier);
}
```

#### 1.4 New Function: verify_world_proof()

```rust
fn verify_world_proof(
    state: &RegistrationState,
    proof: &serde_json::Value,
    pubkey_b64: &str,
) -> Result<String, String> {
    let merkle_root = proof["merkle_root"].as_str().ok_or("missing merkle_root")?;
    let nullifier_hash = proof["nullifier_hash"].as_str().ok_or("missing nullifier_hash")?;
    let proof_str = proof["proof"].as_str().ok_or("missing proof")?;

    // Compute expected signal_hash from pubkey
    let expected_signal_hash = hash_to_field(pubkey_b64.as_bytes());

    let body = serde_json::json!({
        "nullifier_hash": nullifier_hash,
        "merkle_root": merkle_root,
        "proof": proof_str,
        "action": state.world_action,
        "signal_hash": format!("0x{}", hex::encode(&expected_signal_hash)),
    });

    // Blocking HTTP call to World API
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .post(format!("{}/{}", WORLD_VERIFY_URL, state.world_app_id))
        .json(&body)
        .send()
        .map_err(|e| format!("World API request failed: {e}"))?;

    let resp_json: serde_json::Value = resp
        .json()
        .map_err(|e| format!("World API parse failed: {e}"))?;

    if resp_json["success"].as_bool() == Some(true) {
        Ok(nullifier_hash.to_string())
    } else {
        let detail = resp_json["detail"].as_str().unwrap_or("unknown error");
        Err(format!("World ID rejected: {detail}"))
    }
}

fn hash_to_field(input: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let hash = Keccak256::digest(input);
    let mut result = [0u8; 32];
    // Right shift by 8 bits: set first byte to 0, copy remaining 31 bytes
    result[1..].copy_from_slice(&hash[..31]);
    result
}
```

#### 1.5 Extend RegistrationState

```rust
pub struct RegistrationState {
    // ... existing fields ...
    pub human_only: bool,
    pub world_app_id: String,
    pub world_action: String,
}

struct RegistrationInner {
    // ... existing fields ...
    used_nullifiers: HashSet<String>,   // NEW: World ID nullifier tracking
}
```

#### 1.6 Extend boringtun-cli main.rs

In the payment server setup block, read new env vars:

```rust
let human_only = std::env::var("BT_HUMAN_ONLY")
    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    .unwrap_or(false);

let world_app_id = std::env::var("BT_WORLD_APP_ID")
    .unwrap_or_default();

let world_action = std::env::var("BT_WORLD_ACTION")
    .unwrap_or_else(|_| "veil-vpn-connect".to_string());
```

Pass to `RegistrationState::new()`.

### Phase 2: Client - IDKit Integration

#### 2.1 Install Dependencies

```bash
cd client && npm install @worldcoin/idkit
```

#### 2.2 New Tauri Command: get_rp_context

For hackathon/demo: hardcode rp_context generation in client (signing_key in env var).
For production: separate backend server.

**File**: `client/src-tauri/src/lib.rs`

```rust
#[tauri::command]
async fn get_rp_context() -> Result<serde_json::Value, String> {
    // For demo: call our VPN server's new /v1/rp-context endpoint
    // Server generates the RP signature with its signing_key
    let resp = reqwest::get(format!("{}/v1/rp-context", API_BASE))
        .await
        .map_err(|e| format!("Failed to get rp_context: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("Parse error: {e}"))?;
    Ok(resp)
}
```

#### 2.3 Frontend: World ID Verification Flow

**File**: `client/src/App.tsx`

```tsx
import { IDKitWidget, VerificationLevel } from '@worldcoin/idkit';

// New state
const [worldProof, setWorldProof] = useState(null);
const [verifying, setVerifying] = useState(false);

// Modified handleClick for human-only nodes
const handleClick = async () => {
  if (status === "connected" || status === "error") {
    // ... existing disconnect logic ...
  } else if (status === "disconnected") {
    if (selectedServer.humanOnly && !worldProof) {
      // Need World ID verification first
      setVerifying(true);
      return;
    }
    // ... existing connect logic, but include worldProof in register call ...
  }
};

// IDKit widget (rendered when verifying)
{verifying && (
  <IDKitWidget
    app_id="app_xxxxx"
    action="veil-vpn-connect"
    signal={wireguardPubkeyB64}
    verification_level={VerificationLevel.Orb}
    handleVerify={async (result) => {
      // Store proof for register call
      setWorldProof({
        merkle_root: result.merkle_root,
        nullifier_hash: result.nullifier_hash,
        proof: result.proof,
      });
      setVerifying(false);
    }}
    onSuccess={() => {
      // Auto-connect after verification
      handleClick();
    }}
  />
)}
```

#### 2.4 Modified Register Call

**File**: `client/src-tauri/src/vpn.rs`

Add world_proof parameter to connect():

```rust
pub async fn connect(
    &mut self,
    app_handle: tauri::AppHandle,
    world_proof: Option<serde_json::Value>,  // NEW
) -> Result<ConnectedInfo, String> {
    // ... existing key generation ...

    let mut body = serde_json::json!({ "public_key": pub_b64 });

    // Attach World ID proof if provided
    if let Some(proof) = world_proof {
        body["world_proof"] = proof;
    }

    // ... existing register POST (body already built) ...
}
```

#### 2.5 Server List with Human-Only Flag

```tsx
interface Server {
  ens: string;
  region: string;
  ip: string;
  humanOnly: boolean;
}

const SERVERS: Server[] = [
  { ens: "ethglobal.veilvpn.eth", region: "EU", ip: "37.27.29.160", humanOnly: true },
  { ens: "silk.veilvpn.eth", region: "US", ip: "37.27.29.160", humanOnly: false },
  { ens: "ghost.veilvpn.eth", region: "APAC", ip: "37.27.29.160", humanOnly: false },
];
```

UI'da human-only node'lar badge ile işaretlenir.

### Phase 3: Server - RP Context Endpoint (Optional)

For World ID v4, the server needs to generate RP signatures. Add a new endpoint:

```
GET /v1/rp-context?action=veil-vpn-connect
```

Returns:
```json
{
  "rp_id": "rp_xxxxx",
  "nonce": "0x...",
  "created_at": 1712188800,
  "expires_at": 1712189100,
  "signature": "0x..."
}
```

This requires the signing_key (secp256k1 private key from Developer Portal) to live on the server. Implementation uses the same secp256k1 + keccak256 signing already in the codebase.

---

## File Change Summary

| File | Change |
|------|--------|
| `protocol/boringtun/src/device/http_api.rs` | Add world_proof parsing, verify_world_proof(), hash_to_field(), nullifier storage, human_only config |
| `protocol/boringtun/src/device/mod.rs` | No changes needed |
| `protocol/boringtun-cli/src/main.rs` | Read BT_HUMAN_ONLY, BT_WORLD_APP_ID, BT_WORLD_ACTION env vars, pass to RegistrationState |
| `protocol/boringtun/Cargo.toml` | reqwest already included (payment feature) |
| `client/package.json` | Add @worldcoin/idkit dependency |
| `client/src/App.tsx` | Add IDKit widget, world proof state, human-only flow |
| `client/src-tauri/src/vpn.rs` | Add world_proof parameter to connect(), include in register body |
| `client/src-tauri/src/lib.rs` | Pass world_proof from frontend to connect() |

## Dependencies

### Server (already available via payment feature)
- `reqwest` (blocking) - for World API calls
- `sha3` (Keccak256) - for hash_to_field
- `serde_json` - already used

### Client
- `@worldcoin/idkit` - new npm dependency

## Environment Variables (Server)

```bash
# Existing
BT_PAYMENT_SERVER=1
BT_HTTP_BIND=0.0.0.0:8080
BT_PUBLIC_IP=37.27.29.160

# New
BT_HUMAN_ONLY=1                          # Enable human verification
BT_WORLD_APP_ID=app_xxxxx                # From developer.world.org
BT_WORLD_ACTION=veil-vpn-connect         # Action string for nullifier scoping
```

## Testing

1. Create app at developer.world.org → get app_id
2. Deploy server with BT_HUMAN_ONLY=1
3. Client: click Connect on human-only node → IDKit QR appears
4. Scan with World App → proof generated
5. Client sends proof in register request
6. Server verifies with World API → peer created
7. Same user tries again → nullifier blocked

## Security Notes

- signing_key NEVER in client binary - server-side only
- Nullifier deduplication is server's responsibility (World API only verifies proof validity)
- signal = WireGuard pubkey → proof cannot be reused with different key
- Nullifiers persist in-memory (lost on restart) - acceptable for hackathon, use SQLite for prod
- hash_to_field uses Keccak-256 (Ethereum variant), NOT NIST SHA3-256
