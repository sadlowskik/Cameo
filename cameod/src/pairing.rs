//! One-time device enrollment for Cameo Link.
//!
//! Pairing codes and issued device credentials are 256-bit OS-random values.
//! The hub stores only SHA-256 digests, compares them without data-dependent
//! early exit, bounds pending offers, and consumes a code exactly once.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};

const PAIRING_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_PENDING_PAIRINGS: usize = 64;
const MAX_PAIRING_LABEL_BYTES: usize = 128;

#[derive(Debug, Clone, Serialize)]
pub struct PairingOffer {
    pub code: String,
    pub expires_in_seconds: u64,
}

struct PendingPairing {
    digest: [u8; 32],
    label: String,
    expires: Instant,
}

pub struct PairingStore {
    pending: Mutex<Vec<PendingPairing>>,
}

impl Default for PairingStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PairingStore {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
        }
    }

    pub fn begin(&self, label: &str) -> Result<PairingOffer, String> {
        self.begin_at(label, Instant::now())
    }

    fn begin_at(&self, label: &str, now: Instant) -> Result<PairingOffer, String> {
        let label = label.trim();
        if label.is_empty()
            || label.len() > MAX_PAIRING_LABEL_BYTES
            || label.chars().any(char::is_control)
        {
            return Err(format!(
                "pairing label must be 1..={MAX_PAIRING_LABEL_BYTES} bytes with no control characters"
            ));
        }
        let code = random_secret()?;
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|offer| offer.expires > now);
        if pending.len() >= MAX_PENDING_PAIRINGS {
            return Err("too many pending pairings; wait for one to expire".into());
        }
        pending.push(PendingPairing {
            digest: hash_secret(&code),
            label: label.to_string(),
            expires: now + PAIRING_TTL,
        });
        Ok(PairingOffer {
            code,
            expires_in_seconds: PAIRING_TTL.as_secs(),
        })
    }

    /// Consume a live code and return its operator label. The caller should only
    /// invoke this after validating the rest of the registration body: a bad
    /// callback must not burn a legitimate one-time code.
    pub fn consume(&self, code: &str) -> Result<String, String> {
        self.consume_at(code, Instant::now())
    }

    fn consume_at(&self, code: &str, now: Instant) -> Result<String, String> {
        if code.len() != 64 || !code.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("invalid or expired pairing code".into());
        }
        let wanted = hash_secret(&code.to_ascii_lowercase());
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|offer| offer.expires > now);
        let mut found = None;
        for (index, offer) in pending.iter().enumerate() {
            if ct_digest_eq(&offer.digest, &wanted) {
                found = Some(index);
            }
        }
        let Some(index) = found else {
            return Err("invalid or expired pairing code".into());
        };
        Ok(pending.remove(index).label)
    }
}

pub fn issue_device_credential() -> Result<String, String> {
    random_secret()
}

pub fn hash_secret(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

pub fn credential_matches(stored: &[u8; 32], presented: &str) -> bool {
    ct_digest_eq(stored, &hash_secret(presented))
}

fn ct_digest_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right.iter())
        .fold(0u8, |different, (a, b)| different | (a ^ b))
        == 0
}

fn random_secret() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| "operating system randomness unavailable".to_string())?;
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_code_is_random_single_use_and_digest_backed() {
        let store = PairingStore::new();
        let first = store.begin("office laptop").unwrap();
        let second = store.begin("render box").unwrap();
        assert_eq!(first.code.len(), 64);
        assert_ne!(first.code, second.code);
        assert_eq!(store.consume(&first.code).unwrap(), "office laptop");
        assert!(store.consume(&first.code).is_err());
        assert_eq!(store.pending.lock().unwrap().len(), 1);
    }

    #[test]
    fn wrong_and_expired_codes_have_the_same_public_error() {
        let store = PairingStore::new();
        let now = Instant::now();
        let offer = store.begin_at("node", now).unwrap();
        let wrong = store.consume_at(&"0".repeat(64), now).unwrap_err();
        let expired = store
            .consume_at(&offer.code, now + PAIRING_TTL + Duration::from_secs(1))
            .unwrap_err();
        assert_eq!(wrong, expired);
    }

    #[test]
    fn device_credentials_are_random_and_match_only_their_digest() {
        let credential = issue_device_credential().unwrap();
        let digest = hash_secret(&credential);
        assert!(credential_matches(&digest, &credential));
        assert!(!credential_matches(&digest, &"f".repeat(64)));
    }
}
