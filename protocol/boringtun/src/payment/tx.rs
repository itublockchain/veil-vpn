use k256::ecdsa::SigningKey;
use sha3::{Digest, Keccak256};

/// EIP-1559 (Type 2) transaction.
pub struct Eip1559Tx {
    pub chain_id: u64,
    pub nonce: u64,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_gas: u128,
    pub gas_limit: u64,
    pub to: [u8; 20],
    pub value: u128,
    pub data: Vec<u8>,
}

impl Eip1559Tx {
    fn rlp_fields(&self) -> Vec<Vec<u8>> {
        vec![
            rlp_encode_uint(self.chain_id),
            rlp_encode_uint(self.nonce),
            rlp_encode_u128(self.max_priority_fee_per_gas),
            rlp_encode_u128(self.max_fee_per_gas),
            rlp_encode_uint(self.gas_limit),
            rlp_encode_bytes(&self.to),
            rlp_encode_u128(self.value),
            rlp_encode_bytes(&self.data),
            rlp_encode_list(&[]), // empty access list
        ]
    }

    fn signing_hash(&self) -> [u8; 32] {
        let rlp_payload = rlp_encode_list(&self.rlp_fields());
        let mut preimage = vec![0x02u8];
        preimage.extend_from_slice(&rlp_payload);
        Keccak256::digest(&preimage).into()
    }

    /// Sign the transaction and return raw bytes ready for eth_sendRawTransaction.
    pub fn sign(&self, key: &SigningKey) -> Vec<u8> {
        let hash = self.signing_hash();
        let (signature, recovery_id) = key
            .sign_prehash_recoverable(&hash)
            .expect("signing failed");

        let sig_bytes = signature.to_bytes();
        let r = &sig_bytes[..32];
        let s = &sig_bytes[32..];
        // EIP-1559 uses raw parity (0 or 1), NOT legacy v (27/28)
        let y_parity = recovery_id.to_byte();

        let mut fields = self.rlp_fields();
        fields.push(rlp_encode_uint(y_parity as u64));
        fields.push(rlp_encode_bytes(r));
        fields.push(rlp_encode_bytes(s));

        let rlp_signed = rlp_encode_list(&fields);
        let mut raw_tx = vec![0x02u8]; // EIP-1559 type prefix
        raw_tx.extend_from_slice(&rlp_signed);
        raw_tx
    }
}

// === RLP Encoding ===

fn rlp_encode_uint(value: u64) -> Vec<u8> {
    if value == 0 {
        return vec![0x80]; // empty string = zero
    }
    let be = value.to_be_bytes();
    let start = be.iter().position(|&b| b != 0).unwrap_or(7);
    rlp_encode_bytes(&be[start..])
}

fn rlp_encode_u128(value: u128) -> Vec<u8> {
    if value == 0 {
        return vec![0x80];
    }
    let be = value.to_be_bytes();
    let start = be.iter().position(|&b| b != 0).unwrap_or(15);
    rlp_encode_bytes(&be[start..])
}

fn rlp_encode_bytes(data: &[u8]) -> Vec<u8> {
    if data.len() == 1 && data[0] < 0x80 {
        return vec![data[0]];
    }
    if data.len() <= 55 {
        let mut out = vec![0x80 + data.len() as u8];
        out.extend_from_slice(data);
        out
    } else {
        let len_bytes = encode_length(data.len());
        let mut out = vec![0xb7 + len_bytes.len() as u8];
        out.extend_from_slice(&len_bytes);
        out.extend_from_slice(data);
        out
    }
}

fn rlp_encode_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload: Vec<u8> = items.iter().flat_map(|i| i.iter().copied()).collect();
    if payload.len() <= 55 {
        let mut out = vec![0xc0 + payload.len() as u8];
        out.extend_from_slice(&payload);
        out
    } else {
        let len_bytes = encode_length(payload.len());
        let mut out = vec![0xf7 + len_bytes.len() as u8];
        out.extend_from_slice(&len_bytes);
        out.extend_from_slice(&payload);
        out
    }
}

fn encode_length(len: usize) -> Vec<u8> {
    let be = (len as u64).to_be_bytes();
    let start = be.iter().position(|&b| b != 0).unwrap_or(7);
    be[start..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rlp_encode_zero() {
        assert_eq!(rlp_encode_uint(0), vec![0x80]);
    }

    #[test]
    fn test_rlp_encode_small_int() {
        assert_eq!(rlp_encode_uint(1), vec![0x01]);
        assert_eq!(rlp_encode_uint(127), vec![0x7f]);
    }

    #[test]
    fn test_rlp_encode_medium_int() {
        // 128 = 0x80, needs length prefix
        assert_eq!(rlp_encode_uint(128), vec![0x81, 0x80]);
        assert_eq!(rlp_encode_uint(256), vec![0x82, 0x01, 0x00]);
    }

    #[test]
    fn test_rlp_encode_empty_bytes() {
        assert_eq!(rlp_encode_bytes(&[]), vec![0x80]);
    }

    #[test]
    fn test_rlp_encode_empty_list() {
        assert_eq!(rlp_encode_list(&[]), vec![0xc0]);
    }

    #[test]
    fn test_eip1559_sign_produces_valid_raw_tx() {
        let key = SigningKey::from_bytes((&[1u8; 32]).into()).unwrap();
        let tx = Eip1559Tx {
            chain_id: 5042002,
            nonce: 0,
            max_priority_fee_per_gas: 1_000_000,
            max_fee_per_gas: 2_000_000,
            gas_limit: 60_000,
            to: [0xAA; 20],
            value: 0,
            data: vec![0x09, 0x5e, 0xa7, 0xb3], // approve selector
        };
        let raw = tx.sign(&key);
        assert_eq!(raw[0], 0x02); // EIP-1559 type prefix
        assert!(raw.len() > 100); // reasonable size for a signed tx
    }
}
