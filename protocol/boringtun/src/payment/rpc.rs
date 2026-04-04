use serde_json::{json, Value};

pub struct RpcClient {
    client: reqwest::blocking::Client,
    url: String,
}

impl RpcClient {
    pub fn new(url: &str) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to create RPC HTTP client");
        Self {
            client,
            url: url.to_string(),
        }
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let body = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        });
        let resp = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .map_err(|e| format!("RPC request failed: {}", e))?;
        let json: Value = resp
            .json()
            .map_err(|e| format!("RPC response parse failed: {}", e))?;
        if let Some(err) = json.get("error") {
            return Err(format!("RPC error: {}", err));
        }
        Ok(json["result"].clone())
    }

    /// eth_call — execute a read-only call, returns raw bytes.
    pub fn eth_call(&self, to: &[u8; 20], data: &[u8]) -> Result<Vec<u8>, String> {
        let result = self.call(
            "eth_call",
            json!([
                {
                    "to": format!("0x{}", hex::encode(to)),
                    "data": format!("0x{}", hex::encode(data)),
                },
                "latest"
            ]),
        )?;
        let hex_str = result.as_str().ok_or("eth_call result not a string")?;
        decode_hex(hex_str)
    }

    /// Get the transaction count (nonce) for an address.
    pub fn get_nonce(&self, addr: &[u8; 20]) -> Result<u64, String> {
        let result = self.call(
            "eth_getTransactionCount",
            json!([format!("0x{}", hex::encode(addr)), "latest"]),
        )?;
        parse_hex_u64(result.as_str().unwrap_or("0x0"))
    }

    /// Get current gas price.
    pub fn gas_price(&self) -> Result<u128, String> {
        let result = self.call("eth_gasPrice", json!([]))?;
        parse_hex_u128(result.as_str().unwrap_or("0x0"))
    }

    /// Get max priority fee per gas.
    pub fn max_priority_fee(&self) -> Result<u128, String> {
        let result = self.call("eth_maxPriorityFeePerGas", json!([]))?;
        parse_hex_u128(result.as_str().unwrap_or("0x0"))
    }

    /// Estimate gas for a transaction.
    pub fn estimate_gas(
        &self,
        from: &[u8; 20],
        to: &[u8; 20],
        data: &[u8],
    ) -> Result<u64, String> {
        let result = self.call(
            "eth_estimateGas",
            json!([{
                "from": format!("0x{}", hex::encode(from)),
                "to": format!("0x{}", hex::encode(to)),
                "data": format!("0x{}", hex::encode(data)),
            }]),
        )?;
        parse_hex_u64(result.as_str().unwrap_or("0x0"))
    }

    /// Send a signed raw transaction.
    pub fn send_raw_tx(&self, raw_tx: &[u8]) -> Result<String, String> {
        let result = self.call(
            "eth_sendRawTransaction",
            json!([format!("0x{}", hex::encode(raw_tx))]),
        )?;
        result
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "no tx hash in response".to_string())
    }

    /// Get transaction receipt. Returns None if not yet mined.
    pub fn get_receipt(&self, tx_hash: &str) -> Result<Option<Value>, String> {
        let result = self.call("eth_getTransactionReceipt", json!([tx_hash]))?;
        if result.is_null() {
            Ok(None)
        } else {
            Ok(Some(result))
        }
    }

    /// Wait for a transaction receipt, polling every 2 seconds.
    /// Returns the receipt status ("0x1" = success).
    pub fn wait_for_receipt(&self, tx_hash: &str, max_attempts: u32) -> Result<bool, String> {
        for _ in 0..max_attempts {
            if let Some(receipt) = self.get_receipt(tx_hash)? {
                let status = receipt["status"].as_str().unwrap_or("0x0");
                return Ok(status == "0x1");
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        Err("timeout waiting for receipt".to_string())
    }
}

/// Read a u256 from eth_call result bytes (last 32 bytes).
pub fn parse_u256_result(data: &[u8]) -> u128 {
    if data.len() < 32 {
        return 0;
    }
    // Take the last 16 bytes (u128 max) from the 32-byte word
    let offset = data.len() - 16;
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&data[offset..]);
    u128::from_be_bytes(bytes)
}

fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.is_empty() {
        return Ok(vec![]);
    }
    hex::decode(s).map_err(|e| format!("hex decode: {}", e))
}

fn parse_hex_u64(s: &str) -> Result<u64, String> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(s, 16).map_err(|e| format!("parse u64: {}", e))
}

fn parse_hex_u128(s: &str) -> Result<u128, String> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.is_empty() {
        return Ok(0);
    }
    u128::from_str_radix(s, 16).map_err(|e| format!("parse u128: {}", e))
}
