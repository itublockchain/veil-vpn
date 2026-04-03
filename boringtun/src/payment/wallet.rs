use hkdf::Hkdf;
use k256::ecdsa::{RecoveryId, Signature, SigningKey, VerifyingKey};
use sha2::Sha256;
use sha3::{Digest, Keccak256};

const HKDF_SALT: &[u8] = b"boringtun-payment-v1";
const HKDF_INFO: &[u8] = b"secp256k1-signing-key";

pub struct PaymentWallet {
    signing_key: SigningKey,
    ethereum_address: [u8; 20],
}

impl PaymentWallet {
    /// Derive a secp256k1 payment wallet from a WireGuard X25519 private key.
    pub fn from_wireguard_key(x25519_private_bytes: &[u8; 32]) -> Self {
        let hk = Hkdf::<Sha256>::new(Some(HKDF_SALT), x25519_private_bytes);
        let mut derived = [0u8; 32];
        hk.expand(HKDF_INFO, &mut derived)
            .expect("32 bytes is valid HKDF-SHA256 output length");

        let signing_key = SigningKey::from_bytes((&derived).into())
            .expect("HKDF output is valid secp256k1 scalar");

        let ethereum_address = Self::compute_address(&signing_key);

        Self {
            signing_key,
            ethereum_address,
        }
    }

    fn compute_address(signing_key: &SigningKey) -> [u8; 20] {
        let verify_key = VerifyingKey::from(signing_key);
        let pubkey_point = verify_key.to_encoded_point(false);
        let pubkey_bytes = pubkey_point.as_bytes();
        // Skip the 0x04 prefix byte, hash the 64 bytes of x||y
        let hash = Keccak256::digest(&pubkey_bytes[1..]);
        let mut address = [0u8; 20];
        address.copy_from_slice(&hash[12..]);
        address
    }

    pub fn ethereum_address(&self) -> [u8; 20] {
        self.ethereum_address
    }

    pub fn ethereum_address_hex(&self) -> String {
        format!("0x{}", hex::encode(self.ethereum_address))
    }

    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }

    /// Sign a raw 32-byte digest and return (v, r, s).
    pub fn sign_digest(&self, digest: &[u8; 32]) -> (u8, [u8; 32], [u8; 32]) {
        let (signature, recovery_id) = self
            .signing_key
            .sign_prehash_recoverable(digest)
            .expect("signing should not fail");

        let sig_bytes = signature.to_bytes();
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&sig_bytes[..32]);
        s.copy_from_slice(&sig_bytes[32..]);
        let v = recovery_id.to_byte() + 27;

        (v, r, s)
    }

    /// Recover Ethereum address from a signature over a digest.
    pub fn recover_address(digest: &[u8; 32], v: u8, r: &[u8; 32], s: &[u8; 32]) -> Option<[u8; 20]> {
        let recovery_id = RecoveryId::from_byte(v.checked_sub(27)?)?;

        let mut sig_bytes = [0u8; 64];
        sig_bytes[..32].copy_from_slice(r);
        sig_bytes[32..].copy_from_slice(s);
        let signature = Signature::from_bytes((&sig_bytes).into()).ok()?;

        let recovered_key =
            VerifyingKey::recover_from_prehash(digest, &signature, recovery_id).ok()?;

        let pubkey_point = recovered_key.to_encoded_point(false);
        let pubkey_bytes = pubkey_point.as_bytes();
        let hash = Keccak256::digest(&pubkey_bytes[1..]);
        let mut address = [0u8; 20];
        address.copy_from_slice(&hash[12..]);
        Some(address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_derivation() {
        let wg_key = [42u8; 32];
        let wallet1 = PaymentWallet::from_wireguard_key(&wg_key);
        let wallet2 = PaymentWallet::from_wireguard_key(&wg_key);
        assert_eq!(wallet1.ethereum_address(), wallet2.ethereum_address());
    }

    #[test]
    fn test_different_keys_different_addresses() {
        let wallet1 = PaymentWallet::from_wireguard_key(&[1u8; 32]);
        let wallet2 = PaymentWallet::from_wireguard_key(&[2u8; 32]);
        assert_ne!(wallet1.ethereum_address(), wallet2.ethereum_address());
    }

    #[test]
    fn test_address_format() {
        let wallet = PaymentWallet::from_wireguard_key(&[42u8; 32]);
        let hex = wallet.ethereum_address_hex();
        assert!(hex.starts_with("0x"));
        assert_eq!(hex.len(), 42); // "0x" + 40 hex chars
    }

    #[test]
    fn test_sign_and_recover() {
        let wallet = PaymentWallet::from_wireguard_key(&[42u8; 32]);
        let digest = Keccak256::digest(b"test message");
        let digest_arr: [u8; 32] = digest.into();

        let (v, r, s) = wallet.sign_digest(&digest_arr);
        let recovered = PaymentWallet::recover_address(&digest_arr, v, &r, &s);

        assert_eq!(recovered, Some(wallet.ethereum_address()));
    }

    #[test]
    fn test_invalid_signature_recovery() {
        let digest = [0u8; 32];
        let r = [0u8; 32];
        let s = [0u8; 32];
        // v=0 is invalid (must be 27 or 28)
        assert!(PaymentWallet::recover_address(&digest, 0, &r, &s).is_none());
    }
}
