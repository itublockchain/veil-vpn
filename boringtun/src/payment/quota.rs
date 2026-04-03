use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;

pub const DEFAULT_QUOTA_BYTES: u64 = 10 * 1024 * 1024; // 10 MB

pub struct BandwidthQuota {
    remaining_bytes: AtomicU64,
    blocked: AtomicBool,
    total_consumed: AtomicU64,
    payment_count: AtomicU64,
    used_nonces: Mutex<HashSet<[u8; 32]>>,
}

impl BandwidthQuota {
    pub fn new(initial_bytes: u64) -> Self {
        Self {
            remaining_bytes: AtomicU64::new(initial_bytes),
            blocked: AtomicBool::new(false),
            total_consumed: AtomicU64::new(0),
            payment_count: AtomicU64::new(0),
            used_nonces: Mutex::new(HashSet::new()),
        }
    }

    /// Consume bytes from quota. Returns true if allowed, false if quota just exhausted.
    pub fn consume(&self, bytes: u64) -> bool {
        let remaining = self.remaining_bytes.load(Ordering::Relaxed);
        if remaining == 0 {
            return false;
        }
        if bytes > remaining {
            self.remaining_bytes.store(0, Ordering::Relaxed);
            self.total_consumed.fetch_add(remaining, Ordering::Relaxed);
            self.blocked.store(true, Ordering::Release);
            false
        } else {
            self.remaining_bytes.fetch_sub(bytes, Ordering::Relaxed);
            self.total_consumed.fetch_add(bytes, Ordering::Relaxed);
            true
        }
    }

    /// Credit quota after successful payment.
    pub fn credit(&self, bytes: u64) {
        self.remaining_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.payment_count.fetch_add(1, Ordering::Relaxed);
        self.blocked.store(false, Ordering::Release);
    }

    /// Check if a nonce has been used. If not, records it and returns true.
    /// Returns false if the nonce was already used (replay attempt).
    pub fn check_and_record_nonce(&self, nonce: &[u8; 32]) -> bool {
        self.used_nonces.lock().insert(*nonce)
    }

    pub fn is_blocked(&self) -> bool {
        self.blocked.load(Ordering::Acquire)
    }

    pub fn remaining(&self) -> u64 {
        self.remaining_bytes.load(Ordering::Relaxed)
    }

    pub fn total_consumed(&self) -> u64 {
        self.total_consumed.load(Ordering::Relaxed)
    }

    pub fn payment_count(&self) -> u64 {
        self.payment_count.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_quota() {
        let q = BandwidthQuota::new(DEFAULT_QUOTA_BYTES);
        assert_eq!(q.remaining(), DEFAULT_QUOTA_BYTES);
        assert!(!q.is_blocked());
        assert_eq!(q.total_consumed(), 0);
        assert_eq!(q.payment_count(), 0);
    }

    #[test]
    fn test_consume_within_quota() {
        let q = BandwidthQuota::new(1000);
        assert!(q.consume(500));
        assert_eq!(q.remaining(), 500);
        assert!(!q.is_blocked());
        assert_eq!(q.total_consumed(), 500);
    }

    #[test]
    fn test_consume_exceeds_quota() {
        let q = BandwidthQuota::new(1000);
        assert!(q.consume(500));
        assert!(!q.consume(600)); // exceeds remaining 500
        assert_eq!(q.remaining(), 0);
        assert!(q.is_blocked());
        assert_eq!(q.total_consumed(), 1000);
    }

    #[test]
    fn test_consume_when_blocked() {
        let q = BandwidthQuota::new(100);
        assert!(!q.consume(200));
        assert!(q.is_blocked());
        assert!(!q.consume(1)); // still blocked
    }

    #[test]
    fn test_credit_unblocks() {
        let q = BandwidthQuota::new(100);
        q.consume(200);
        assert!(q.is_blocked());

        q.credit(DEFAULT_QUOTA_BYTES);
        assert!(!q.is_blocked());
        assert_eq!(q.remaining(), DEFAULT_QUOTA_BYTES);
        assert_eq!(q.payment_count(), 1);

        assert!(q.consume(1000));
    }

    #[test]
    fn test_nonce_replay_protection() {
        let q = BandwidthQuota::new(1000);
        let nonce = [0xAB; 32];

        assert!(q.check_and_record_nonce(&nonce));  // first use: OK
        assert!(!q.check_and_record_nonce(&nonce)); // replay: rejected

        let nonce2 = [0xCD; 32];
        assert!(q.check_and_record_nonce(&nonce2)); // different nonce: OK
    }
}
