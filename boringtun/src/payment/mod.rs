pub mod quota;
pub mod wallet;
pub mod eip3009;
pub mod protocol;

/// Payment configuration.
#[derive(Clone)]
pub struct PaymentConfig {
    pub chain_id: u64,
    pub usdc_contract: [u8; 20],
    pub amount_per_quota: u64,      // USDC smallest unit per quota (e.g. 10000 = $0.01)
    pub quota_bytes: u64,           // bytes per payment (e.g. 10MB)
    pub usdc_name: String,
    pub usdc_version: String,
    /// When true, this node enforces bandwidth quotas and sends PaymentRequired.
    /// When false (client mode), this node only responds to PaymentRequired signals.
    pub is_server: bool,
}

impl Default for PaymentConfig {
    fn default() -> Self {
        // Base Sepolia testnet defaults
        let mut usdc_contract = [0u8; 20];
        let bytes = hex::decode("036CbD53842c5426634e7929541eC2318f3dCF7e").unwrap();
        usdc_contract.copy_from_slice(&bytes);

        // Determine role from BT_PAYMENT_SERVER env var.
        // Server: BT_PAYMENT_SERVER=1 (enforces quotas, sends PaymentRequired)
        // Client: absent or any other value (responds to PaymentRequired, auto-signs)
        let is_server = std::env::var("BT_PAYMENT_SERVER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        Self {
            chain_id: 84532, // Base Sepolia
            usdc_contract,
            amount_per_quota: 10_000, // 0.01 USDC
            quota_bytes: 10 * 1024 * 1024, // 10 MB
            usdc_name: "USD Coin".to_string(),
            usdc_version: "2".to_string(),
            is_server,
        }
    }
}
