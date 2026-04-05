use sha3::{Digest, Keccak256};
use serde::{Deserialize, Serialize};

const ENS_UNIVERSAL_RESOLVER: &str = "0xeeeeeeee14d718c2b47d9923deab1335e144eeee";
const RPC_URL: &str = "https://1rpc.io/sepolia";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnNodeInfo {
    pub ens_name: String,
    pub is_human: bool,
    pub endpoint: String,
    pub http: String,
    pub public_key: String,
    pub metadata: String,
}

pub async fn resolve_vpn_node(ens_name: &str) -> Result<VpnNodeInfo, String> {
    let keys = ["vpn.ishuman", "vpn.publickey", "vpn.endpoint", "vpn.http", "vpn.metadata"];

    // Parallel resolve all keys
    let futures: Vec<_> = keys.iter().map(|key| get_ens_text(ens_name, key)).collect();
    let results = futures::future::join_all(futures).await;

    let mut values = Vec::new();
    for (key, result) in keys.iter().zip(results) {
        let val = result.unwrap_or_else(|e| {
            log::warn!("[ens] failed {key}: {e}");
            String::new()
        });
        log::info!("[ens] {ens_name} {key} = '{val}'");
        values.push(val);
    }

    if values[2].is_empty() && values[3].is_empty() {
        return Err(format!("No VPN records found for {ens_name}"));
    }

    Ok(VpnNodeInfo {
        ens_name: ens_name.to_string(),
        is_human: values[0] == "true" || values[0] == "1",
        public_key: values[1].clone(),
        endpoint: values[2].clone(),
        http: values[3].clone(),
        metadata: values[4].clone(),
    })
}

async fn get_ens_text(name: &str, key: &str) -> Result<String, String> {
    let dns_name = dns_encode(name);
    let node = namehash(name);

    // Build text(bytes32,string) calldata - exactly matching viem's encoding
    let mut text_call = Vec::new();
    text_call.extend_from_slice(&[0x59, 0xd1, 0xd4, 0x3c]); // text selector
    text_call.extend_from_slice(&node);                        // bytes32 namehash
    text_call.extend_from_slice(&uint256(64));                 // offset to string = 0x40
    text_call.extend_from_slice(&uint256(key.len()));          // string length
    text_call.extend_from_slice(key.as_bytes());               // string data
    pad32_vec(&mut text_call);

    // Build resolve(bytes,bytes) calldata - matching viem's ABI encoding
    // abi.encode(bytes dns_name, bytes text_call)
    let dns_len = dns_name.len();
    let dns_padded = ceil32(dns_len);
    let text_len = text_call.len();
    let text_padded = ceil32(text_len);

    // Offsets: first bytes at 0x40, second bytes at 0x40 + 0x20 + dns_padded
    let first_offset = 64usize;
    let second_offset = first_offset + 32 + dns_padded;

    let mut calldata = Vec::new();
    calldata.extend_from_slice(&[0x90, 0x61, 0xb9, 0x23]); // resolve selector
    calldata.extend_from_slice(&uint256(first_offset));      // offset to dns bytes
    calldata.extend_from_slice(&uint256(second_offset));     // offset to text_call bytes
    // dns bytes
    calldata.extend_from_slice(&uint256(dns_len));           // length
    calldata.extend_from_slice(&dns_name);
    calldata.resize(calldata.len() + (dns_padded - dns_len), 0); // pad
    // text_call bytes
    calldata.extend_from_slice(&uint256(text_len));          // length
    calldata.extend_from_slice(&text_call);
    calldata.resize(calldata.len() + (text_padded - text_len), 0); // pad

    let data_hex = format!("0x{}", hex::encode(&calldata));

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_call",
        "params": [{
            "to": ENS_UNIVERSAL_RESOLVER,
            "data": data_hex,
        }, "latest"],
        "id": 1
    });

    let client = reqwest::Client::builder()
        .user_agent("VeilVPN/1.0")
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| format!("HTTP error: {e}"))?;

    let resp: serde_json::Value = client
        .post(RPC_URL)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("RPC failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("RPC parse failed: {e}"))?;

    log::info!("[ens] resolve {name} key={key} rpc_resp={resp}");

    if resp.get("error").is_some() {
        log::warn!("[ens] RPC error for {name}/{key}: {}", resp["error"]);
        return Ok(String::new());
    }

    let result_hex = resp["result"].as_str().unwrap_or("0x");
    log::info!("[ens] result hex len={} first100={}", result_hex.len(), &result_hex[..result_hex.len().min(100)]);
    if result_hex == "0x" || result_hex.len() < 10 {
        return Ok(String::new());
    }

    let bytes = hex::decode(result_hex.strip_prefix("0x").unwrap_or(result_hex))
        .map_err(|_| "hex decode failed")?;

    // Decode resolve return: (bytes, address)
    // [0..32] = offset to bytes
    // [32..64] = address (resolver)
    // [offset..offset+32] = bytes length
    // [offset+32..] = bytes data (= ABI-encoded text() return)
    decode_resolve_text_result(&bytes)
}

fn decode_resolve_text_result(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() < 64 {
        return Ok(String::new());
    }

    let data_offset = read_usize(&bytes[0..32]);
    if data_offset + 32 > bytes.len() {
        return Ok(String::new());
    }

    let data_len = read_usize(&bytes[data_offset..data_offset + 32]);
    if data_len == 0 || data_offset + 32 + data_len > bytes.len() {
        return Ok(String::new());
    }

    let inner = &bytes[data_offset + 32..data_offset + 32 + data_len];

    // inner = ABI-encoded (string): offset(32) + length(32) + data
    if inner.len() < 64 {
        return Ok(String::new());
    }

    let str_offset = read_usize(&inner[0..32]);
    if str_offset + 32 > inner.len() {
        return Ok(String::new());
    }

    let str_len = read_usize(&inner[str_offset..str_offset + 32]);
    if str_len == 0 || str_offset + 32 + str_len > inner.len() {
        return Ok(String::new());
    }

    let text = String::from_utf8_lossy(&inner[str_offset + 32..str_offset + 32 + str_len]);
    Ok(text.to_string())
}

fn namehash(name: &str) -> [u8; 32] {
    let mut node = [0u8; 32];
    if name.is_empty() {
        return node;
    }
    for label in name.rsplit('.') {
        let label_hash: [u8; 32] = Keccak256::digest(label.as_bytes()).into();
        let mut buf = [0u8; 64];
        buf[..32].copy_from_slice(&node);
        buf[32..].copy_from_slice(&label_hash);
        node = Keccak256::digest(&buf).into();
    }
    node
}

fn dns_encode(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in name.split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out
}

fn uint256(val: usize) -> [u8; 32] {
    let mut word = [0u8; 32];
    let bytes = (val as u64).to_be_bytes();
    word[24..].copy_from_slice(&bytes);
    word
}

fn ceil32(n: usize) -> usize {
    ((n + 31) / 32) * 32
}

fn pad32_vec(buf: &mut Vec<u8>) {
    let rem = buf.len() % 32;
    if rem != 0 {
        buf.resize(buf.len() + (32 - rem), 0);
    }
}

fn read_usize(bytes: &[u8]) -> usize {
    if bytes.len() < 32 { return 0; }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[24..32]);
    u64::from_be_bytes(buf) as usize
}
