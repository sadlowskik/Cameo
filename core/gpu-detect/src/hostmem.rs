//! Host memory facts.
//!
//! The planner cannot decide "offload the rest to host RAM" without knowing
//! whether any host RAM exists to offload into. On a 4 GB APU that question is
//! the whole ballgame: the GPU carve-out and the "host RAM" the planner wants
//! to spill into are the same physical DIMM.
//!
//! Parsing is pure; the `/proc/meminfo` read is Linux-only and lives in
//! [`crate::collect`].

use serde::{Deserialize, Serialize};

/// What the machine has to work with, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostMemory {
    /// `MemTotal` — all RAM the kernel manages. Note this already *excludes* a
    /// BIOS UMA carve-out, which is why an APU's carve-out is additive rather
    /// than a slice of this number.
    pub total_bytes: u64,
    /// `MemAvailable` — what can be handed out without swapping. This, not
    /// `MemTotal`, is the honest offload budget on a running system.
    pub available_bytes: u64,
}

impl HostMemory {
    /// Build from kibibyte counts as `/proc/meminfo` reports them.
    pub fn from_kib(total_kib: u64, available_kib: u64) -> Self {
        Self {
            total_bytes: total_kib.saturating_mul(1024),
            available_bytes: available_kib.saturating_mul(1024),
        }
    }
}

/// Parse `/proc/meminfo` text into [`HostMemory`].
///
/// Returns `None` unless `MemTotal` is present. `MemAvailable` is absent on
/// kernels older than 3.14; there we fall back to `MemFree`, which understates
/// the budget — the safe direction to be wrong in.
pub fn parse_meminfo(text: &str) -> Option<HostMemory> {
    let mut total = None;
    let mut available = None;
    let mut free = None;

    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let kib: Option<u64> = rest.split_whitespace().next().and_then(|v| v.parse().ok());
        match key.trim() {
            "MemTotal" => total = kib,
            "MemAvailable" => available = kib,
            "MemFree" => free = kib,
            _ => {}
        }
    }

    let total = total?;
    Some(HostMemory::from_kib(total, available.or(free).unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMINFO: &str = "\
MemTotal:        3921160 kB
MemFree:          210284 kB
MemAvailable:    2554120 kB
Buffers:           41232 kB
";

    #[test]
    fn reads_total_and_available() {
        let m = parse_meminfo(MEMINFO).unwrap();
        assert_eq!(m.total_bytes, 3_921_160 * 1024);
        assert_eq!(m.available_bytes, 2_554_120 * 1024);
    }

    #[test]
    fn falls_back_to_memfree_on_old_kernels() {
        let old = "MemTotal:        3921160 kB\nMemFree:          210284 kB\n";
        let m = parse_meminfo(old).unwrap();
        assert_eq!(m.available_bytes, 210_284 * 1024);
    }

    #[test]
    fn no_memtotal_is_none() {
        assert!(parse_meminfo("Buffers: 100 kB\n").is_none());
    }
}
