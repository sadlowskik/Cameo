//! Role-based API credentials and deployment posture.
//!
//! Cameo separates **who may use a model** from **who may manipulate the GPU**.
//! That split is the whole security story for sharing a box:
//!
//! - a **Consumer** credential reaches inference (`/v1`) only — a friend or tenant
//!   you serve, who must never be able to touch VRAM or the fleet;
//! - an **Operator** credential reaches the control surface (`/api`, `/hub`,
//!   dispatch, start/stop/evict) — a privileged member who runs the box.
//!
//! [`Posture`] then decides whether "co-located" counts as trusted: on a
//! **self-host** box a local harness may get keyless operator power (the
//! privileged local socket); on a **multi-tenant** box that socket is off and only
//! an operator credential may manipulate VRAM, so one tenant cannot evict another
//! tenant's resident model.
//!
//! Everything here is pure and unit-tested; the daemon consults a [`KeyRing`] to
//! gate each route.

use std::path::Path;

use serde::Deserialize;

/// What a credential is allowed to do. `Operator` is a superset of `Consumer` —
/// an operator may also run inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Operator,
    Consumer,
}

/// One named API key with its role. The label is for the operator's own
/// bookkeeping — revoke "friend-bob" without rotating everyone else.
#[derive(Debug, Clone)]
pub struct ApiKey {
    pub key: String,
    pub role: Role,
    pub label: String,
}

/// Deployment posture — who the box trusts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posture {
    /// One operator owns the box; a co-located harness may be granted keyless GPU
    /// control via the privileged local socket.
    SelfHost,
    /// Many users share the box; the privileged local socket is disabled, and only
    /// an operator credential may manipulate VRAM.
    MultiTenant,
}

impl Posture {
    pub fn parse(s: &str) -> Option<Posture> {
        match s.to_ascii_lowercase().as_str() {
            "self-host" | "selfhost" | "self" | "single" | "single-tenant" => {
                Some(Posture::SelfHost)
            }
            "multi-tenant" | "multitenant" | "multi" | "enterprise" => Some(Posture::MultiTenant),
            _ => None,
        }
    }

    /// Whether a co-located harness may be granted keyless operator power.
    pub fn allows_local_harness(self) -> bool {
        matches!(self, Posture::SelfHost)
    }
}

/// The set of credentials cameod accepts, each carrying a role.
#[derive(Debug, Clone, Default)]
pub struct KeyRing {
    keys: Vec<ApiKey>,
}

impl KeyRing {
    pub fn new(keys: Vec<ApiKey>) -> Self {
        KeyRing { keys }
    }

    /// Constant-time compare, so a near-miss key does not leak how much matched via
    /// response timing. Only length is observable, and that is not a secret.
    fn ct_eq(a: &str, b: &str) -> bool {
        a.len() == b.len()
            && a.bytes()
                .zip(b.bytes())
                .fold(0u8, |acc, (x, y)| acc | (x ^ y))
                == 0
    }

    /// The role of a presented bearer token, if it matches a configured key.
    pub fn role_of(&self, presented: Option<&str>) -> Option<Role> {
        let p = presented?;
        self.keys
            .iter()
            .find(|k| Self::ct_eq(p, &k.key))
            .map(|k| k.role)
    }

    /// Whether any operator credential exists — so operator routes must authenticate.
    /// When false, the operator surface is open (loopback dev only).
    pub fn requires_operator(&self) -> bool {
        self.keys.iter().any(|k| k.role == Role::Operator)
    }

    /// Whether any consumer credential exists — so `/v1` must authenticate. When
    /// false, inference is open (matching a keyless dev daemon).
    pub fn requires_consumer(&self) -> bool {
        self.keys.iter().any(|k| k.role == Role::Consumer)
    }

    /// Does a presented token grant operator (VRAM-manipulating) access?
    pub fn is_operator(&self, presented: Option<&str>) -> bool {
        self.role_of(presented) == Some(Role::Operator)
    }

    /// Does a presented token grant at least consumer (inference) access? Operators
    /// qualify too, since operator is a superset of consumer.
    pub fn is_consumer_or_better(&self, presented: Option<&str>) -> bool {
        self.role_of(presented).is_some()
    }
}

/// One entry in a JSON keys file.
#[derive(Deserialize)]
struct KeyFileEntry {
    key: String,
    role: String,
    #[serde(default)]
    label: String,
}

/// Parse a JSON keys file into role-tagged keys. Shape:
/// `[{ "key": "…", "role": "operator|consumer", "label": "friend-bob" }]`.
pub fn load_keys_file(path: &Path) -> Result<Vec<ApiKey>, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let entries: Vec<KeyFileEntry> =
        serde_json::from_str(&raw).map_err(|e| format!("parsing {}: {e}", path.display()))?;
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let role = match e.role.to_ascii_lowercase().as_str() {
            "operator" | "admin" => Role::Operator,
            "consumer" | "user" | "inference" => Role::Consumer,
            other => return Err(format!("unknown role '{other}' in {}", path.display())),
        };
        if e.key.trim().is_empty() {
            return Err(format!("empty key in {}", path.display()));
        }
        out.push(ApiKey {
            key: e.key,
            role,
            label: e.label,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring() -> KeyRing {
        KeyRing::new(vec![
            ApiKey {
                key: "op-key".into(),
                role: Role::Operator,
                label: "korbin".into(),
            },
            ApiKey {
                key: "friend".into(),
                role: Role::Consumer,
                label: "bob".into(),
            },
        ])
    }

    #[test]
    fn operator_key_gets_operator_and_consumer_access() {
        let r = ring();
        assert!(r.is_operator(Some("op-key")));
        assert!(r.is_consumer_or_better(Some("op-key")));
    }

    #[test]
    fn consumer_key_gets_inference_but_not_operator() {
        let r = ring();
        // The whole point: a friend can use models but cannot touch VRAM/the fleet.
        assert!(!r.is_operator(Some("friend")));
        assert!(r.is_consumer_or_better(Some("friend")));
    }

    #[test]
    fn unknown_or_missing_key_gets_nothing() {
        let r = ring();
        assert!(!r.is_operator(Some("nope")));
        assert!(!r.is_consumer_or_better(Some("nope")));
        assert!(!r.is_operator(None));
        assert_eq!(r.role_of(None), None);
    }

    #[test]
    fn requires_flags_reflect_configured_roles() {
        let r = ring();
        assert!(r.requires_operator());
        assert!(r.requires_consumer());
        // An operator-only ring leaves inference open (requires_consumer false).
        let ops = KeyRing::new(vec![ApiKey {
            key: "x".into(),
            role: Role::Operator,
            label: String::new(),
        }]);
        assert!(ops.requires_operator());
        assert!(!ops.requires_consumer());
        // An empty ring gates nothing (dev/loopback).
        let empty = KeyRing::default();
        assert!(!empty.requires_operator());
        assert!(!empty.requires_consumer());
    }

    #[test]
    fn posture_parses_its_aliases_and_gates_the_local_harness() {
        assert_eq!(Posture::parse("self-host"), Some(Posture::SelfHost));
        assert_eq!(Posture::parse("enterprise"), Some(Posture::MultiTenant));
        assert_eq!(Posture::parse("nonsense"), None);
        assert!(Posture::SelfHost.allows_local_harness());
        assert!(!Posture::MultiTenant.allows_local_harness());
    }

    #[test]
    fn ct_eq_matches_only_identical_strings() {
        assert!(KeyRing::ct_eq("abc", "abc"));
        assert!(!KeyRing::ct_eq("abc", "abd"));
        assert!(!KeyRing::ct_eq("abc", "abcd"));
    }
}
