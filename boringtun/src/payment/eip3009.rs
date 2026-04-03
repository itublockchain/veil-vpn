use sha3::{Digest, Keccak256};

use crate::payment::wallet::PaymentWallet;

/// EIP-712 domain separator for USDC contract.
pub struct Eip712Domain {
    pub name: String,
    pub version: String,
    pub chain_id: u64,
    pub verifying_contract: [u8; 20],
}

/// EIP-3009 TransferWithAuthorization parameters.
pub struct TransferAuthorization {
    pub from: [u8; 20],
    pub to: [u8; 20],
    pub value: u64,
    pub valid_after: u64,
    pub valid_before: u64,
    pub nonce: [u8; 32],
}

/// Signed EIP-3009 authorization ready to submit.
pub struct SignedAuthorization {
    pub auth: TransferAuthorization,
    pub v: u8,
    pub r: [u8; 32],
    pub s: [u8; 32],
}

// keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")
fn eip712_domain_type_hash() -> [u8; 32] {
    Keccak256::digest(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    )
    .into()
}

// keccak256("TransferWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)")
fn transfer_authorization_type_hash() -> [u8; 32] {
    Keccak256::digest(
        b"TransferWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)",
    )
    .into()
}

fn compute_domain_separator(domain: &Eip712Domain) -> [u8; 32] {
    let mut data = Vec::with_capacity(5 * 32);
    data.extend_from_slice(&eip712_domain_type_hash());
    data.extend_from_slice(&Keccak256::digest(domain.name.as_bytes()));
    data.extend_from_slice(&Keccak256::digest(domain.version.as_bytes()));
    data.extend_from_slice(&abi_encode_u256(domain.chain_id as u128));
    data.extend_from_slice(&abi_encode_address(&domain.verifying_contract));
    Keccak256::digest(&data).into()
}

fn compute_struct_hash(auth: &TransferAuthorization) -> [u8; 32] {
    let mut data = Vec::with_capacity(7 * 32);
    data.extend_from_slice(&transfer_authorization_type_hash());
    data.extend_from_slice(&abi_encode_address(&auth.from));
    data.extend_from_slice(&abi_encode_address(&auth.to));
    data.extend_from_slice(&abi_encode_u256(auth.value as u128));
    data.extend_from_slice(&abi_encode_u256(auth.valid_after as u128));
    data.extend_from_slice(&abi_encode_u256(auth.valid_before as u128));
    // nonce is bytes32, already 32 bytes, left-padded
    data.extend_from_slice(&auth.nonce);
    Keccak256::digest(&data).into()
}

/// Compute the EIP-712 digest for signing.
pub fn compute_eip712_digest(domain: &Eip712Domain, auth: &TransferAuthorization) -> [u8; 32] {
    let domain_separator = compute_domain_separator(domain);
    let struct_hash = compute_struct_hash(auth);

    let mut data = Vec::with_capacity(2 + 32 + 32);
    data.push(0x19);
    data.push(0x01);
    data.extend_from_slice(&domain_separator);
    data.extend_from_slice(&struct_hash);
    Keccak256::digest(&data).into()
}

/// Sign an EIP-3009 TransferWithAuthorization.
pub fn sign_authorization(
    wallet: &PaymentWallet,
    domain: &Eip712Domain,
    auth: TransferAuthorization,
) -> SignedAuthorization {
    let digest = compute_eip712_digest(domain, &auth);
    let (v, r, s) = wallet.sign_digest(&digest);
    SignedAuthorization { auth, v, r, s }
}

/// Verify an EIP-3009 signed authorization. Returns the recovered signer address.
pub fn verify_authorization(
    domain: &Eip712Domain,
    signed: &SignedAuthorization,
) -> Option<[u8; 20]> {
    let digest = compute_eip712_digest(domain, &signed.auth);
    PaymentWallet::recover_address(&digest, signed.v, &signed.r, &signed.s)
}

/// ABI-encode an address as a 32-byte word (left-padded with zeros).
fn abi_encode_address(addr: &[u8; 20]) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(addr);
    word
}

/// ABI-encode a u128 value as a 32-byte big-endian word.
fn abi_encode_u256(value: u128) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[16..].copy_from_slice(&value.to_be_bytes());
    word
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_domain() -> Eip712Domain {
        Eip712Domain {
            name: "USD Coin".to_string(),
            version: "2".to_string(),
            chain_id: 84532, // Base Sepolia
            verifying_contract: hex_to_address("0x036CbD53842c5426634e7929541eC2318f3dCF7e"),
        }
    }

    fn hex_to_address(s: &str) -> [u8; 20] {
        let s = s.strip_prefix("0x").unwrap_or(s);
        let bytes = hex::decode(s).unwrap();
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&bytes);
        addr
    }

    #[test]
    fn test_sign_and_verify() {
        let wallet = PaymentWallet::from_wireguard_key(&[42u8; 32]);
        let domain = test_domain();

        let auth = TransferAuthorization {
            from: wallet.ethereum_address(),
            to: hex_to_address("0x1111111111111111111111111111111111111111"),
            value: 10_000, // 0.01 USDC
            valid_after: 0,
            valid_before: 1_000_000_000,
            nonce: [7u8; 32],
        };

        let signed = sign_authorization(&wallet, &domain, auth);
        let recovered = verify_authorization(&domain, &signed);

        assert_eq!(recovered, Some(wallet.ethereum_address()));
    }

    #[test]
    fn test_wrong_signer_rejected() {
        let wallet1 = PaymentWallet::from_wireguard_key(&[1u8; 32]);
        let wallet2 = PaymentWallet::from_wireguard_key(&[2u8; 32]);
        let domain = test_domain();

        let auth = TransferAuthorization {
            from: wallet1.ethereum_address(), // claims to be wallet1
            to: hex_to_address("0x1111111111111111111111111111111111111111"),
            value: 10_000,
            valid_after: 0,
            valid_before: 1_000_000_000,
            nonce: [7u8; 32],
        };

        // But signed by wallet2
        let signed = sign_authorization(&wallet2, &domain, auth);
        let recovered = verify_authorization(&domain, &signed);

        // Recovered address should NOT match `from`
        assert_ne!(recovered, Some(signed.auth.from));
    }

    #[test]
    fn test_tampered_value_rejected() {
        let wallet = PaymentWallet::from_wireguard_key(&[42u8; 32]);
        let domain = test_domain();

        let auth = TransferAuthorization {
            from: wallet.ethereum_address(),
            to: hex_to_address("0x1111111111111111111111111111111111111111"),
            value: 10_000,
            valid_after: 0,
            valid_before: 1_000_000_000,
            nonce: [7u8; 32],
        };

        let mut signed = sign_authorization(&wallet, &domain, auth);
        // Tamper with the value
        signed.auth.value = 1;

        let recovered = verify_authorization(&domain, &signed);
        // Recovered address should NOT match original signer
        assert_ne!(recovered, Some(wallet.ethereum_address()));
    }

    #[test]
    fn test_domain_separator_deterministic() {
        let domain1 = test_domain();
        let domain2 = test_domain();
        assert_eq!(
            compute_domain_separator(&domain1),
            compute_domain_separator(&domain2)
        );
    }

    #[test]
    fn test_different_chain_different_domain() {
        let mut domain1 = test_domain();
        let mut domain2 = test_domain();
        domain2.chain_id = 1; // mainnet
        assert_ne!(
            compute_domain_separator(&domain1),
            compute_domain_separator(&domain2)
        );
    }
}
