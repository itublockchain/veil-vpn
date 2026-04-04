use std::convert::TryInto;
use std::net::Ipv4Addr;

pub const PAYMENT_GATEWAY_IP: Ipv4Addr = Ipv4Addr::new(169, 254, 254, 1);
pub const PAYMENT_PORT: u16 = 7078;

// Message types: Server → Client
pub const MSG_PAYMENT_REQUIRED: u8 = 0x01;
pub const MSG_PAYMENT_ACCEPTED: u8 = 0x02;
pub const MSG_QUOTA_STATUS: u8 = 0x03;

// Message types: Client → Server
pub const MSG_PAYMENT_SUBMIT: u8 = 0x81;
pub const MSG_QUOTA_QUERY: u8 = 0x82;

/// PaymentRequired payload sent from server to client.
#[derive(Debug)]
pub struct PaymentRequired {
    pub amount_usdc: u64,
    pub nonce: [u8; 32],
    pub recipient: [u8; 20],
    pub deadline: u64,
    pub chain_id: u64,
    pub usdc_contract: [u8; 20],
}

/// PaymentSubmit payload sent from client to server.
#[derive(Debug)]
pub struct PaymentSubmit {
    pub from: [u8; 20],
    pub to: [u8; 20],
    pub value: u64,
    pub valid_after: u64,
    pub valid_before: u64,
    pub nonce: [u8; 32],
    pub v: u8,
    pub r: [u8; 32],
    pub s: [u8; 32],
}

/// PaymentAccepted payload sent from server to client.
#[derive(Debug)]
pub struct PaymentAccepted {
    pub new_quota_bytes: u64,
}

// === Serialization ===

impl PaymentRequired {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(3 + 8 + 32 + 20 + 8 + 8 + 20);
        let payload_len = 8 + 32 + 20 + 8 + 8 + 20; // 96 bytes
        buf.push(MSG_PAYMENT_REQUIRED);
        buf.extend_from_slice(&(payload_len as u16).to_be_bytes());
        buf.extend_from_slice(&self.amount_usdc.to_be_bytes());
        buf.extend_from_slice(&self.nonce);
        buf.extend_from_slice(&self.recipient);
        buf.extend_from_slice(&self.deadline.to_be_bytes());
        buf.extend_from_slice(&self.chain_id.to_be_bytes());
        buf.extend_from_slice(&self.usdc_contract);
        buf
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < 3 || data[0] != MSG_PAYMENT_REQUIRED {
            return None;
        }
        let len = u16::from_be_bytes([data[1], data[2]]) as usize;
        if data.len() < 3 + len || len < 96 {
            return None;
        }
        let p = &data[3..];
        let amount_usdc = u64::from_be_bytes(p[0..8].try_into().ok()?);
        let mut nonce = [0u8; 32];
        nonce.copy_from_slice(&p[8..40]);
        let mut recipient = [0u8; 20];
        recipient.copy_from_slice(&p[40..60]);
        let deadline = u64::from_be_bytes(p[60..68].try_into().ok()?);
        let chain_id = u64::from_be_bytes(p[68..76].try_into().ok()?);
        let mut usdc_contract = [0u8; 20];
        usdc_contract.copy_from_slice(&p[76..96]);
        Some(Self { amount_usdc, nonce, recipient, deadline, chain_id, usdc_contract })
    }
}

impl PaymentSubmit {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(3 + 20 + 20 + 8 + 8 + 8 + 32 + 1 + 32 + 32);
        let payload_len = 20 + 20 + 8 + 8 + 8 + 32 + 1 + 32 + 32; // 161 bytes
        buf.push(MSG_PAYMENT_SUBMIT);
        buf.extend_from_slice(&(payload_len as u16).to_be_bytes());
        buf.extend_from_slice(&self.from);
        buf.extend_from_slice(&self.to);
        buf.extend_from_slice(&self.value.to_be_bytes());
        buf.extend_from_slice(&self.valid_after.to_be_bytes());
        buf.extend_from_slice(&self.valid_before.to_be_bytes());
        buf.extend_from_slice(&self.nonce);
        buf.push(self.v);
        buf.extend_from_slice(&self.r);
        buf.extend_from_slice(&self.s);
        buf
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < 3 || data[0] != MSG_PAYMENT_SUBMIT {
            return None;
        }
        let len = u16::from_be_bytes([data[1], data[2]]) as usize;
        if data.len() < 3 + len || len < 161 {
            return None;
        }
        let p = &data[3..];
        let mut from = [0u8; 20];
        from.copy_from_slice(&p[0..20]);
        let mut to = [0u8; 20];
        to.copy_from_slice(&p[20..40]);
        let value = u64::from_be_bytes(p[40..48].try_into().ok()?);
        let valid_after = u64::from_be_bytes(p[48..56].try_into().ok()?);
        let valid_before = u64::from_be_bytes(p[56..64].try_into().ok()?);
        let mut nonce = [0u8; 32];
        nonce.copy_from_slice(&p[64..96]);
        let v = p[96];
        let mut r = [0u8; 32];
        r.copy_from_slice(&p[97..129]);
        let mut s = [0u8; 32];
        s.copy_from_slice(&p[129..161]);
        Some(Self { from, to, value, valid_after, valid_before, nonce, v, r, s })
    }
}

impl PaymentAccepted {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(3 + 8);
        buf.push(MSG_PAYMENT_ACCEPTED);
        buf.extend_from_slice(&8u16.to_be_bytes());
        buf.extend_from_slice(&self.new_quota_bytes.to_be_bytes());
        buf
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < 3 || data[0] != MSG_PAYMENT_ACCEPTED {
            return None;
        }
        let len = u16::from_be_bytes([data[1], data[2]]) as usize;
        if data.len() < 3 + len || len < 8 {
            return None;
        }
        let new_quota_bytes = u64::from_be_bytes(data[3..11].try_into().ok()?);
        Some(Self { new_quota_bytes })
    }
}

// === IP/UDP Packet Construction ===

/// Build a complete IPv4/UDP packet wrapping a TLV payload.
pub fn build_signal_packet(
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    payload: &[u8],
) -> Vec<u8> {
    let udp_len = 8 + payload.len();
    let total_len = 20 + udp_len;
    assert!(total_len <= u16::MAX as usize, "Payment signal packet exceeds maximum IPv4 packet size");
    let mut packet = vec![0u8; total_len];

    // IPv4 header (20 bytes)
    packet[0] = 0x45; // version=4, IHL=5
    packet[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
    packet[6] = 0x40; // Flags: Don't Fragment (DF) set, no fragmentation offset
    packet[8] = 64; // TTL
    packet[9] = 17; // Protocol: UDP
    packet[12..16].copy_from_slice(&src_ip.octets());
    packet[16..20].copy_from_slice(&dst_ip.octets());

    // IP header checksum (computed with checksum field zeroed)
    let cksum = ip_checksum(&packet[..20]);
    packet[10..12].copy_from_slice(&cksum.to_be_bytes());

    // UDP header (8 bytes)
    let udp = &mut packet[20..];
    udp[0..2].copy_from_slice(&PAYMENT_PORT.to_be_bytes()); // src port
    udp[2..4].copy_from_slice(&PAYMENT_PORT.to_be_bytes()); // dst port
    udp[4..6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    // udp checksum [6..8] = 0 (optional for IPv4, packet travels inside WireGuard tunnel)

    // Payload
    udp[8..8 + payload.len()].copy_from_slice(payload);

    packet
}

/// Check if a decapsulated IPv4 packet is a payment signal.
pub fn is_payment_signal(packet: &[u8]) -> bool {
    if packet.len() < 20 || (packet[0] >> 4) != 4 {
        return false;
    }
    let src = Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]);
    let dst = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
    src == PAYMENT_GATEWAY_IP || dst == PAYMENT_GATEWAY_IP
}

/// Extract the UDP payload from a payment signal IP packet.
pub fn extract_signal_payload(packet: &[u8]) -> Option<&[u8]> {
    if packet.len() < 20 {
        return None;
    }
    let ihl = (packet[0] & 0x0f) as usize * 4;
    let protocol = packet[9];
    if protocol != 17 {
        return None; // not UDP
    }
    if packet.len() < ihl + 8 {
        return None;
    }
    let udp_len = u16::from_be_bytes([packet[ihl + 4], packet[ihl + 5]]) as usize;
    if udp_len < 8 || packet.len() < ihl + udp_len {
        return None;
    }
    Some(&packet[ihl + 8..ihl + udp_len])
}

/// Extract source IPv4 address from a packet.
pub fn src_ipv4(packet: &[u8]) -> Option<Ipv4Addr> {
    if packet.len() < 20 {
        return None;
    }
    Some(Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]))
}

/// Extract destination IPv4 address from a packet.
pub fn dst_ipv4(packet: &[u8]) -> Option<Ipv4Addr> {
    if packet.len() < 20 {
        return None;
    }
    Some(Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]))
}

fn ip_checksum(header: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for i in (0..header.len()).step_by(2) {
        let word = if i + 1 < header.len() {
            u16::from_be_bytes([header[i], header[i + 1]])
        } else {
            u16::from_be_bytes([header[i], 0])
        };
        sum = sum.wrapping_add(word as u32);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payment_required_roundtrip() {
        let msg = PaymentRequired {
            amount_usdc: 10_000,
            nonce: [7u8; 32],
            recipient: [0xAA; 20],
            deadline: 1_000_000,
            chain_id: 84532,
            usdc_contract: [0xBB; 20],
        };
        let encoded = msg.encode();
        let decoded = PaymentRequired::decode(&encoded).unwrap();
        assert_eq!(decoded.amount_usdc, 10_000);
        assert_eq!(decoded.nonce, [7u8; 32]);
        assert_eq!(decoded.recipient, [0xAA; 20]);
        assert_eq!(decoded.chain_id, 84532);
    }

    #[test]
    fn test_payment_submit_roundtrip() {
        let msg = PaymentSubmit {
            from: [1u8; 20],
            to: [2u8; 20],
            value: 10_000,
            valid_after: 100,
            valid_before: 200,
            nonce: [3u8; 32],
            v: 28,
            r: [4u8; 32],
            s: [5u8; 32],
        };
        let encoded = msg.encode();
        let decoded = PaymentSubmit::decode(&encoded).unwrap();
        assert_eq!(decoded.from, [1u8; 20]);
        assert_eq!(decoded.value, 10_000);
        assert_eq!(decoded.v, 28);
    }

    #[test]
    fn test_payment_accepted_roundtrip() {
        let msg = PaymentAccepted { new_quota_bytes: 10 * 1024 * 1024 };
        let encoded = msg.encode();
        let decoded = PaymentAccepted::decode(&encoded).unwrap();
        assert_eq!(decoded.new_quota_bytes, 10 * 1024 * 1024);
    }

    #[test]
    fn test_signal_packet_detected() {
        let payload = PaymentRequired {
            amount_usdc: 10_000,
            nonce: [0u8; 32],
            recipient: [0u8; 20],
            deadline: 0,
            chain_id: 1,
            usdc_contract: [0u8; 20],
        }.encode();
        let packet = build_signal_packet(PAYMENT_GATEWAY_IP, Ipv4Addr::new(10, 0, 0, 2), &payload);
        assert!(is_payment_signal(&packet));

        let extracted = extract_signal_payload(&packet).unwrap();
        assert_eq!(extracted, payload.as_slice());
    }

    #[test]
    fn test_normal_packet_not_signal() {
        // Fake a normal IP packet (src=10.0.0.2, dst=8.8.8.8)
        let mut packet = vec![0u8; 28];
        packet[0] = 0x45;
        packet[9] = 17; // UDP
        packet[12..16].copy_from_slice(&[10, 0, 0, 2]);
        packet[16..20].copy_from_slice(&[8, 8, 8, 8]);
        assert!(!is_payment_signal(&packet));
    }
}
