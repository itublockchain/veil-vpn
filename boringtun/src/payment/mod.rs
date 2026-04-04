pub mod quota;
pub mod wallet;
pub mod eip3009;
pub mod protocol;
pub mod settlement;
pub mod rpc;
pub mod tx;
pub mod deposit;

/// Payment configuration.
#[derive(Clone)]
pub struct PaymentConfig {
    pub chain_id: u64,
    pub usdc_contract: [u8; 20],
    pub amount_per_quota: u64,
    pub quota_bytes: u64,
    pub usdc_name: String,
    pub usdc_version: String,
    /// Gateway Wallet contract address (for nanopayment settlement).
    pub gateway_wallet: [u8; 20],
    /// EIP-712 domain name for nanopayment signing.
    pub gateway_name: String,
    /// EIP-712 domain version for nanopayment signing.
    pub gateway_version: String,
    /// Circle Gateway API base URL.
    pub gateway_api_url: String,
    /// Arc RPC URL (for balance checks / deposits).
    pub rpc_url: String,
    /// When true, this node enforces bandwidth quotas and sends PaymentRequired.
    /// When false (client mode), this node only responds to PaymentRequired signals.
    pub is_server: bool,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn hex_to_address(s: &str) -> [u8; 20] {
    let s = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(s).expect("invalid hex address");
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&bytes);
    addr
}

impl Default for PaymentConfig {
    fn default() -> Self {
        let is_server = std::env::var("BT_PAYMENT_SERVER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let chain_id: u64 = env_or("BT_CHAIN_ID", "5042002")
            .parse()
            .unwrap_or(5042002);

        let usdc_contract = hex_to_address(
            &env_or("BT_USDC_CONTRACT", "3600000000000000000000000000000000000000"),
        );

        let gateway_wallet = hex_to_address(
            &env_or("BT_GATEWAY_WALLET", "0077777d7EBA4688BDeF3E311b846F25870A19B9"),
        );

        let quota_mb: u64 = env_or("BT_QUOTA_MB", "10").parse().unwrap_or(10);
        let amount: u64 = env_or("BT_PAYMENT_AMOUNT", "10000").parse().unwrap_or(10_000);

        Self {
            chain_id,
            usdc_contract,
            amount_per_quota: amount,
            quota_bytes: quota_mb * 1024 * 1024,
            usdc_name: "USDC".to_string(),
            usdc_version: "2".to_string(),
            gateway_wallet,
            gateway_name: "GatewayWalletBatched".to_string(),
            gateway_version: "1".to_string(),
            gateway_api_url: env_or(
                "BT_GATEWAY_API",
                "https://gateway-api-testnet.circle.com",
            ),
            rpc_url: env_or("BT_RPC_URL", "https://rpc.testnet.arc.network"),
            is_server,
        }
    }
}

impl PaymentConfig {
    /// EIP-712 domain for USDC contract (used for deposits via depositWithAuthorization).
    pub fn usdc_domain(&self) -> crate::payment::eip3009::Eip712Domain {
        crate::payment::eip3009::Eip712Domain {
            name: self.usdc_name.clone(),
            version: self.usdc_version.clone(),
            chain_id: self.chain_id,
            verifying_contract: self.usdc_contract,
        }
    }

    /// EIP-712 domain for GatewayWalletBatched (used for nanopayment signing).
    pub fn gateway_domain(&self) -> crate::payment::eip3009::Eip712Domain {
        crate::payment::eip3009::Eip712Domain {
            name: self.gateway_name.clone(),
            version: self.gateway_version.clone(),
            chain_id: self.chain_id,
            verifying_contract: self.gateway_wallet,
        }
    }
}
