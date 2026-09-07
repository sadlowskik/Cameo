//! Bounded fixed-window request admission for Cameo's small synchronous HTTP surface.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_IDENTITIES: usize = 4096;

pub const WINDOW_SECONDS: u64 = 60;
pub const PAIRING_REQUESTS_PER_WINDOW: u32 = 10;
pub const INVALID_AUTH_REQUESTS_PER_WINDOW: u32 = 30;
pub const INFERENCE_REQUESTS_PER_WINDOW: u32 = 600;
pub const CONTROL_REQUESTS_PER_WINDOW: u32 = 300;
pub const PUBLIC_REQUESTS_PER_WINDOW: u32 = 300;

struct Window {
    reset_at: Instant,
    count: u32,
}

pub struct RateLimiter {
    windows: Mutex<HashMap<String, Window>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
        }
    }

    pub fn allow(&self, key: &str, limit: u32, window: Duration) -> bool {
        self.allow_at(key, limit, window, Instant::now())
    }

    fn allow_at(&self, key: &str, limit: u32, duration: Duration, now: Instant) -> bool {
        if limit == 0 {
            return false;
        }
        let mut windows = self.windows.lock().unwrap();
        if windows.len() >= MAX_IDENTITIES && !windows.contains_key(key) {
            windows.retain(|_, value| value.reset_at > now);
            if windows.len() >= MAX_IDENTITIES {
                return false;
            }
        }
        let entry = windows.entry(key.to_string()).or_insert(Window {
            reset_at: now + duration,
            count: 0,
        });
        if entry.reset_at <= now {
            entry.reset_at = now + duration;
            entry.count = 0;
        }
        if entry.count >= limit {
            return false;
        }
        entry.count += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_each_identity_and_resets_the_window() {
        let limiter = RateLimiter::new();
        let start = Instant::now();
        let minute = Duration::from_secs(60);
        assert!(limiter.allow_at("a", 2, minute, start));
        assert!(limiter.allow_at("a", 2, minute, start));
        assert!(!limiter.allow_at("a", 2, minute, start));
        assert!(limiter.allow_at("b", 2, minute, start));
        assert!(limiter.allow_at("a", 2, minute, start + minute));
    }
}
