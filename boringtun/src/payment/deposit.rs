use sha3::{Digest, Keccak256};

use super::rpc::{self, RpcClient};
use super::tx::Eip1559Tx;
use super::wallet::PaymentWallet;
use super::PaymentConfig;

// Function selectors (verified keccak256)
const SEL_BALANCE_OF: [u8; 4] = [0x70, 0xa0, 0x82, 0x31];
const SEL_AVAILABLE_BALANCE: [u8; 4] = [0x3c, 0xcb, 0x64, 0xae];
const SEL_DEPOSIT_WITH_AUTH: [u8; 4] = [0x8a, 0x94, 0xd4, 0xfc];

/// Check balances and auto-deposit USDC into Gateway Wallet if needed.
/// Called once at client startup. Blocking.
pub fn auto_deposit(wallet: &PaymentWallet, config: &PaymentConfig) -> Result<String, String> {
    let rpc = RpcClient::new(&config.rpc_url);
    let addr = wallet.ethereum_address();

    // 1. Check USDC balance (ERC-20)
    let usdc_balance = query_balance(&rpc, &config.usdc_contract, &addr)?;
    tracing::info!("USDC balance: {} (raw units)", usdc_balance);

    // 2. Check Gateway available balance
    let gw_balance = query_gateway_balance(&rpc, &config.gateway_wallet, &config.usdc_contract, &addr)?;
    tracing::info!("Gateway balance: {} (raw units)", gw_balance);

    if usdc_balance == 0 && gw_balance == 0 {
        return Err(format!(
            "No USDC available. Fund wallet with USDC on Arc testnet: {}",
            wallet.ethereum_address_hex()
        ));
    }

    if usdc_balance == 0 {
        tracing::info!("No USDC to deposit, Gateway balance already available");
        return Ok("already_deposited".to_string());
    }

    // 3. Deposit USDC into Gateway via depositWithAuthorization.
    //    On Arc, gas is paid in USDC (native token). Reserve some for gas fees
    //    since the ERC-20 balance and native balance share the same pool.
    //    Reserve 1 USDC (1_000_000 raw units at 6 decimals) for gas.
    const GAS_RESERVE: u128 = 100_000; // 0.1 USDC
    let deposit_amount = usdc_balance.saturating_sub(GAS_RESERVE);
    if deposit_amount == 0 {
        return Err("USDC balance too low to deposit (need >1 USDC for gas reserve)".to_string());
    }
    tracing::info!("Depositing {} USDC units into Gateway (reserving {} for gas)...", deposit_amount, GAS_RESERVE);
    let tx_hash = deposit_with_authorization(wallet, config, &rpc, deposit_amount)?;
    tracing::info!("Deposit tx submitted: {}", tx_hash);

    // 4. Wait for confirmation
    let success = rpc.wait_for_receipt(&tx_hash, 30)?;
    if success {
        tracing::info!("Deposit confirmed!");
        Ok(tx_hash)
    } else {
        Err(format!("Deposit tx reverted: {}", tx_hash))
    }
}

fn query_balance(rpc: &RpcClient, token: &[u8; 20], addr: &[u8; 20]) -> Result<u128, String> {
    let mut calldata = Vec::with_capacity(4 + 32);
    calldata.extend_from_slice(&SEL_BALANCE_OF);
    calldata.extend_from_slice(&abi_encode_address(addr));
    let result = rpc.eth_call(token, &calldata)?;
    Ok(rpc::parse_u256_result(&result))
}

fn query_gateway_balance(
    rpc: &RpcClient,
    gateway: &[u8; 20],
    token: &[u8; 20],
    addr: &[u8; 20],
) -> Result<u128, String> {
    let mut calldata = Vec::with_capacity(4 + 64);
    calldata.extend_from_slice(&SEL_AVAILABLE_BALANCE);
    calldata.extend_from_slice(&abi_encode_address(token));
    calldata.extend_from_slice(&abi_encode_address(addr));
    let result = rpc.eth_call(gateway, &calldata)?;
    Ok(rpc::parse_u256_result(&result))
}

fn deposit_with_authorization(
    wallet: &PaymentWallet,
    config: &PaymentConfig,
    rpc: &RpcClient,
    amount: u128,
) -> Result<String, String> {
    let addr = wallet.ethereum_address();

    // Generate random nonce for EIP-3009
    let mut nonce = [0u8; 32];
    use rand_core::{OsRng, RngCore};
    OsRng.fill_bytes(&mut nonce);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let valid_after: u64 = 0;
    let valid_before: u64 = now + 600; // 10 minutes

    // Sign ReceiveWithAuthorization (USDC domain, to=Gateway)
    let receive_type_hash: [u8; 32] = Keccak256::digest(
        b"ReceiveWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)",
    ).into();

    let mut struct_data = Vec::with_capacity(7 * 32);
    struct_data.extend_from_slice(&receive_type_hash);
    struct_data.extend_from_slice(&abi_encode_address(&addr));
    struct_data.extend_from_slice(&abi_encode_address(&config.gateway_wallet));
    struct_data.extend_from_slice(&abi_encode_u256(amount));
    struct_data.extend_from_slice(&abi_encode_u256(valid_after as u128));
    struct_data.extend_from_slice(&abi_encode_u256(valid_before as u128));
    struct_data.extend_from_slice(&nonce);
    let struct_hash: [u8; 32] = Keccak256::digest(&struct_data).into();

    // Domain separator from USDC domain (Domain A)
    let usdc_domain = config.usdc_domain();
    let domain_separator = compute_domain_separator(&usdc_domain);

    let mut digest_input = vec![0x19, 0x01];
    digest_input.extend_from_slice(&domain_separator);
    digest_input.extend_from_slice(&struct_hash);
    let digest: [u8; 32] = Keccak256::digest(&digest_input).into();

    let (v, r, s) = wallet.sign_digest(&digest);

    // Build calldata: depositWithAuthorization(token, from, value, validAfter, validBefore, nonce, v, r, s)
    let mut calldata = Vec::with_capacity(4 + 9 * 32);
    calldata.extend_from_slice(&SEL_DEPOSIT_WITH_AUTH);
    calldata.extend_from_slice(&abi_encode_address(&config.usdc_contract)); // token
    calldata.extend_from_slice(&abi_encode_address(&addr));                 // from
    calldata.extend_from_slice(&abi_encode_u256(amount));                   // value
    calldata.extend_from_slice(&abi_encode_u256(valid_after as u128));      // validAfter
    calldata.extend_from_slice(&abi_encode_u256(valid_before as u128));     // validBefore
    calldata.extend_from_slice(&{                                           // nonce (bytes32)
        let mut w = [0u8; 32];
        w.copy_from_slice(&nonce);
        w
    });
    calldata.extend_from_slice(&abi_encode_u256(v as u128));                // v
    calldata.extend_from_slice(&{                                           // r
        let mut w = [0u8; 32];
        w.copy_from_slice(&r);
        w
    });
    calldata.extend_from_slice(&{                                           // s
        let mut w = [0u8; 32];
        w.copy_from_slice(&s);
        w
    });

    // Get tx params
    let tx_nonce = rpc.get_nonce(&addr)?;
    let priority_fee = rpc.max_priority_fee().unwrap_or(1_000_000);
    let base_fee = rpc.gas_price()?;
    let max_fee = base_fee.saturating_add(priority_fee);

    let gas_limit = rpc
        .estimate_gas(&addr, &config.gateway_wallet, &calldata)
        .map(|g| g * 130 / 100) // 30% buffer
        .unwrap_or(200_000);

    let tx = Eip1559Tx {
        chain_id: config.chain_id,
        nonce: tx_nonce,
        max_priority_fee_per_gas: priority_fee,
        max_fee_per_gas: max_fee,
        gas_limit,
        to: config.gateway_wallet,
        value: 0,
        data: calldata,
    };

    let raw = tx.sign(wallet.signing_key());
    rpc.send_raw_tx(&raw)
}

fn compute_domain_separator(domain: &super::eip3009::Eip712Domain) -> [u8; 32] {
    let type_hash: [u8; 32] = Keccak256::digest(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    ).into();
    let mut data = Vec::with_capacity(5 * 32);
    data.extend_from_slice(&type_hash);
    data.extend_from_slice(&Keccak256::digest(domain.name.as_bytes()));
    data.extend_from_slice(&Keccak256::digest(domain.version.as_bytes()));
    data.extend_from_slice(&abi_encode_u256(domain.chain_id as u128));
    data.extend_from_slice(&abi_encode_address(&domain.verifying_contract));
    Keccak256::digest(&data).into()
}

fn abi_encode_address(addr: &[u8; 20]) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(addr);
    word
}

fn abi_encode_u256(value: u128) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[16..].copy_from_slice(&value.to_be_bytes());
    word
}
