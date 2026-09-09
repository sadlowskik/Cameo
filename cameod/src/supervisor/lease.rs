use std::time::{Instant, SystemTime};

pub(super) const LEASE_TTL_SECS: u64 = 90;

pub(super) fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Lease {
    pub(super) session_id: String,
    pub(super) model: String,
    pub(super) endpoint_id: String,
    #[serde(skip)]
    pub(super) recovered: bool,
    #[serde(default)]
    pub(super) expires_at: u64,
    #[serde(skip)]
    pub(super) renewed_at: Option<Instant>,
}

impl Lease {
    pub(super) fn expired(&self, now: u64) -> bool {
        self.expires_at <= now
            || self
                .renewed_at
                .is_some_and(|at| at.elapsed().as_secs() >= LEASE_TTL_SECS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn wall_clock_and_monotonic_deadlines_both_expire_a_lease() {
        let mut lease = Lease {
            session_id: "s".into(),
            model: "m".into(),
            endpoint_id: "e".into(),
            recovered: false,
            expires_at: 100,
            renewed_at: Some(Instant::now()),
        };
        assert!(lease.expired(100));
        lease.expires_at = u64::MAX;
        lease.renewed_at = Some(Instant::now() - Duration::from_secs(LEASE_TTL_SECS));
        assert!(lease.expired(0));
    }
}
