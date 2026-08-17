//! The versioned GPU compatibility database.
//!
//! Maps AMD `gfx` architecture -> support tier and (for Tier 2) a known-good
//! `HSA_OVERRIDE_GFX_VERSION`. The seed data in `data/overrides.toml` is
//! **illustrative** and is meant to be corrected by real Phase 1 captures.

use crate::error::Error;
use crate::types::Tier;
use serde::Deserialize;
use std::collections::HashMap;

/// The compatibility database, keyed by lowercased `gfx` architecture.
#[derive(Debug, Clone)]
pub struct OverrideDb {
    pub version: u32,
    entries: HashMap<String, OverrideEntry>,
}

/// One database entry for a given architecture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideEntry {
    pub tier: Tier,
    pub hsa_override: Option<String>,
    pub note: Option<String>,
}

/// Bundled seed database, compiled into the binary.
const EMBEDDED: &str = include_str!("../data/overrides.toml");

impl OverrideDb {
    /// Load the compiled-in seed database. Panics only if the bundled TOML is
    /// malformed, which a unit test guards against.
    pub fn embedded() -> Self {
        Self::from_toml(EMBEDDED).expect("bundled overrides.toml must parse")
    }

    /// Parse a database from TOML text (used for tests and user-supplied DBs).
    pub fn from_toml(text: &str) -> Result<Self, Error> {
        let raw: RawDb = toml::from_str(text)?;
        let mut entries = HashMap::with_capacity(raw.entry.len());
        for e in raw.entry {
            let tier = tier_from_u8(e.tier)?;
            entries.insert(
                e.gfx_arch.to_lowercase(),
                OverrideEntry {
                    tier,
                    hsa_override: e.hsa_override,
                    note: e.note,
                },
            );
        }
        Ok(Self {
            version: raw.version,
            entries,
        })
    }

    /// Look up an architecture (case-insensitive).
    pub fn lookup(&self, gfx_arch: &str) -> Option<&OverrideEntry> {
        self.entries.get(&gfx_arch.to_lowercase())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn tier_from_u8(n: u8) -> Result<Tier, Error> {
    match n {
        1 => Ok(Tier::Tier1),
        2 => Ok(Tier::Tier2),
        3 => Ok(Tier::Tier3),
        other => Err(Error::BadTier(other)),
    }
}

#[derive(Debug, Deserialize)]
struct RawDb {
    version: u32,
    #[serde(default)]
    entry: Vec<RawEntry>,
}

#[derive(Debug, Deserialize)]
struct RawEntry {
    gfx_arch: String,
    tier: u8,
    #[serde(default)]
    hsa_override: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_db_parses_and_is_nonempty() {
        let db = OverrideDb::embedded();
        assert_eq!(db.version, 1);
        assert!(!db.is_empty());
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let db = OverrideDb::embedded();
        assert!(db.lookup("GFX1030").is_some());
    }

    #[test]
    fn rejects_bad_tier() {
        let bad = "version = 1\n[[entry]]\ngfx_arch = \"gfx999\"\ntier = 7\n";
        assert!(matches!(OverrideDb::from_toml(bad), Err(Error::BadTier(7))));
    }
}
