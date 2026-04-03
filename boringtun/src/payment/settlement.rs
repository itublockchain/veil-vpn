use serde::{Deserialize, Serialize};

use super::PaymentConfig;
use super::protocol::PaymentSubmit;

#[derive(Debug)]
pub enum SettlementError {
    Http(String),
    Json(String),
    Api { reason: String, message: String },
}

impl std::fmt::Display for SettlementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {}", e),
            Self::Json(e) => write!(f, "JSON error: {}", e),
            Self::Api { reason, message } => write!(f, "API error: {} — {}", reason, message),
        }
    }
}

// === Request types ===

#[derive(Serialize)]
pub struct SettleRequest {
    #[serde(rename = "x402Version")]
    pub x402_version: u32,
    #[serde(rename = "paymentPayload")]
    pub payment_payload: PaymentPayload,
    #[serde(rename = "paymentRequirements")]
    pub payment_requirements: PaymentRequirements,
}

#[derive(Serialize)]
pub struct PaymentPayload {
    #[serde(rename = "x402Version")]
    pub x402_version: u32,
    pub accepted: PaymentRequirements,
    pub payload: PayloadData,
}

#[derive(Serialize)]
pub struct PayloadData {
    pub signature: String,
    pub authorization: AuthorizationData,
}

#[derive(Serialize)]
pub struct AuthorizationData {
    pub from: String,
    pub to: String,
    pub value: String,
    #[serde(rename = "validAfter")]
    pub valid_after: String,
    #[serde(rename = "validBefore")]
    pub valid_before: String,
    pub nonce: String,
}

#[derive(Serialize, Clone)]
pub struct PaymentRequirements {
    pub scheme: String,
    pub network: String,
    pub asset: String,
    pub amount: String,
    #[serde(rename = "payTo")]
    pub pay_to: String,
    #[serde(rename = "maxTimeoutSeconds")]
    pub max_timeout_seconds: u32,
    pub extra: GatewayExtra,
}

#[derive(Serialize, Clone)]
pub struct GatewayExtra {
    pub name: String,
    pub version: String,
    #[serde(rename = "verifyingContract")]
    pub verifying_contract: String,
}

// === Response types ===

#[derive(Deserialize, Debug)]
pub struct SettleResponse {
    pub success: bool,
    pub payer: Option<String>,
    pub transaction: Option<String>,
    pub network: Option<String>,
    // Error fields (Circle returns these when success=false)
    #[serde(rename = "errorReason")]
    pub error_reason: Option<String>,
    #[serde(rename = "errorMessage")]
    pub error_message: Option<String>,
}

// === Client ===

pub struct SettlementClient {
    client: reqwest::blocking::Client,
    base_url: String,
}

impl SettlementClient {
    pub fn new(base_url: &str) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to create HTTP client");
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    pub fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, SettlementError> {
        let url = format!("{}/v1/x402/settle", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(request)
            .send()
            .map_err(|e| SettlementError::Http(e.to_string()))?;

        let body = resp
            .json::<SettleResponse>()
            .map_err(|e| SettlementError::Json(e.to_string()))?;

        Ok(body)
    }
}

// === Helper ===

/// Build a SettleRequest from PaymentConfig and a decoded PaymentSubmit.
pub fn build_settle_request(
    config: &PaymentConfig,
    submit: &PaymentSubmit,
    server_address: &[u8; 20],
) -> SettleRequest {
    let network = format!("eip155:{}", config.chain_id);
    let asset = format!("0x{}", hex::encode(config.usdc_contract));
    let pay_to = format!("0x{}", hex::encode(server_address));

    let extra = GatewayExtra {
        name: config.gateway_name.clone(),
        version: config.gateway_version.clone(),
        verifying_contract: format!("0x{}", hex::encode(config.gateway_wallet)),
    };

    let requirements = PaymentRequirements {
        scheme: "exact".to_string(),
        network: network.clone(),
        asset: asset.clone(),
        amount: submit.value.to_string(),
        pay_to: pay_to.clone(),
        max_timeout_seconds: 30,
        extra: extra.clone(),
    };

    // Signature: r (32) + s (32) + v (1) = 65 bytes
    let mut sig_bytes = Vec::with_capacity(65);
    sig_bytes.extend_from_slice(&submit.r);
    sig_bytes.extend_from_slice(&submit.s);
    sig_bytes.push(submit.v);
    let signature = format!("0x{}", hex::encode(&sig_bytes));

    let authorization = AuthorizationData {
        from: format!("0x{}", hex::encode(submit.from)),
        to: format!("0x{}", hex::encode(submit.to)),
        value: submit.value.to_string(),
        valid_after: submit.valid_after.to_string(),
        valid_before: submit.valid_before.to_string(),
        nonce: format!("0x{}", hex::encode(submit.nonce)),
    };

    let payload = PayloadData {
        signature,
        authorization,
    };

    SettleRequest {
        x402_version: 2,
        payment_payload: PaymentPayload {
            x402_version: 2,
            accepted: requirements.clone(),
            payload,
        },
        payment_requirements: requirements,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_settle_request_serializes() {
        let config = PaymentConfig::default();
        let submit = PaymentSubmit {
            from: [0xAA; 20],
            to: [0xBB; 20],
            value: 10_000,
            valid_after: 0,
            valid_before: 1_000_000_000,
            nonce: [0x07; 32],
            v: 27,
            r: [0x11; 32],
            s: [0x22; 32],
        };
        let server_addr = [0xBB; 20];
        let req = build_settle_request(&config, &submit, &server_addr);

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"x402Version\":2"));
        assert!(json.contains("\"scheme\":\"exact\""));
        assert!(json.contains("eip155:5042002"));
        assert!(json.contains("GatewayWalletBatched"));
        assert!(json.contains("\"value\":\"10000\""));
    }
}
