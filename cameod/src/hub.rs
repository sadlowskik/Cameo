//! The hub registry: the "farm" a fleet of nodes phones home to.
//!
//! This is the HiveOS-shaped inversion of `cameo fleet` (the CLI's *pull* model,
//! which polls a static list of node addresses). Here a node running the
//! [`crate::agent`] dials *out* to a hub and registers itself; the hub keeps the
//! roster, paints one central dashboard, and — because it stored each node's
//! callback address and key — can push work down to a node's ordinary `/api`
//! routes. Same `/api/node` self-description as the pull path; only the initiator
//! flips.
//!
//! The registry is pure in-memory state with the same shape as
//! [`crate::sessions::Board`]: a `Mutex<BTreeMap>`, staleness computed on read,
//! and dead entries pruned. No node cap — the project's funding model is
//! sustainable-open-source, so the self-hosted hub is free and unlimited; a paid
//! hosted tier is a future *mode*, not a limit on this one.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A node counts as "online" if it has phoned home within this window. The agent
/// heartbeats on an interval well under this, so a healthy node stays green and a
/// single missed beat does not flap it offline.
const ONLINE_WINDOW: Duration = Duration::from_secs(45);

/// A node silent for this long is dropped from the farm entirely. Offline nodes
/// linger until here so the dashboard can show them greyed-out (HiveOS-style)
/// rather than vanishing the instant they miss a beat — but the window is bounded
/// so a churn of transient nodes cannot grow the map without limit.
const DROP_AFTER: Duration = Duration::from_secs(15 * 60);

/// What a node sends to `POST /hub/register`. A heartbeat reuses the same shape
/// but may omit the (larger) `node` description.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Registration {
    /// A stable identifier for the node. Empty on first contact → derived from
    /// `name`, then `address`, so a node keeps one row across restarts.
    #[serde(default)]
    pub node_id: String,
    #[serde(default)]
    pub name: String,
    /// `host:port` the hub calls back to reach this node's authenticated `/api`.
    /// This is the node's own console address, as the node believes the hub can
    /// reach it (LAN/VPN today; a cloud relay later).
    pub address: String,
    /// The credential the hub must present (`Authorization: Bearer …`) when it
    /// pushes work to this node's `/api`. `None` when the node runs keyless.
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub cameo_version: String,
    /// The node's `GET /api/node` body, stored verbatim for the dashboard and for
    /// later reconstruction of the placement brain's `Cluster`.
    #[serde(default)]
    pub node: Value,
}

/// One enrolled node plus the timing the roster is computed from.
struct Enrolled {
    reg: Registration,
    last_seen: Instant,
}

/// The farm: every node that has phoned home, keyed by `node_id`.
pub struct Farm {
    inner: Mutex<BTreeMap<String, Enrolled>>,
}

impl Default for Farm {
    fn default() -> Self {
        Self::new()
    }
}

impl Farm {
    pub fn new() -> Self {
        Farm {
            inner: Mutex::new(BTreeMap::new()),
        }
    }

    /// Register or refresh a node, returning the `node_id` the hub assigned (which
    /// the node echoes back on subsequent heartbeats).
    pub fn register(&self, mut reg: Registration) -> String {
        let id = pick_id(&reg);
        reg.node_id = id.clone();
        let now = Instant::now();
        let mut map = self.inner.lock().unwrap();
        prune(&mut map, now);
        map.entry(id.clone())
            .and_modify(|e| {
                e.reg = reg.clone();
                e.last_seen = now;
            })
            .or_insert(Enrolled {
                reg,
                last_seen: now,
            });
        id
    }

    /// Refresh a known node's liveness, and its live description when the beat
    /// carries one. Returns `false` if the node is unknown (dropped for silence,
    /// say), which the agent treats as "re-register".
    pub fn heartbeat(&self, node_id: &str, node: Option<Value>) -> bool {
        let now = Instant::now();
        let mut map = self.inner.lock().unwrap();
        match map.get_mut(node_id) {
            Some(e) => {
                e.last_seen = now;
                if let Some(n) = node {
                    if !n.is_null() {
                        e.reg.node = n;
                    }
                }
                true
            }
            None => false,
        }
    }

    /// Forget a node (admin "remove from farm"). `true` if it was present.
    pub fn remove(&self, node_id: &str) -> bool {
        self.inner.lock().unwrap().remove(node_id).is_some()
    }

    /// Online nodes as `(node_id, stored /api/node description)` pairs — the raw
    /// input the dispatch router reconstructs routable candidates from. Offline
    /// nodes are excluded: you cannot delegate work to a box that is not answering.
    pub fn online_descriptions(&self) -> Vec<(String, Value)> {
        let now = Instant::now();
        let mut map = self.inner.lock().unwrap();
        prune(&mut map, now);
        map.values()
            .filter(|e| now.duration_since(e.last_seen) < ONLINE_WINDOW)
            .map(|e| (e.reg.node_id.clone(), e.reg.node.clone()))
            .collect()
    }

    /// Where and with what credential to reach a node's `/api` for a push, if the
    /// node is still enrolled.
    pub fn push_target(&self, node_id: &str) -> Option<(String, Option<String>)> {
        let map = self.inner.lock().unwrap();
        map.get(node_id)
            .map(|e| (e.reg.address.clone(), e.reg.key.clone()))
    }

    /// The farm as the dashboard renders it: one row per node with online state,
    /// age, and a compact GPU summary lifted from the stored self-description.
    pub fn list(&self) -> Vec<Value> {
        let now = Instant::now();
        let mut map = self.inner.lock().unwrap();
        prune(&mut map, now);
        map.values()
            .map(|e| {
                let age = now.duration_since(e.last_seen);
                let name = if e.reg.name.is_empty() {
                    e.reg.node_id.clone()
                } else {
                    e.reg.name.clone()
                };
                json!({
                    "node_id": e.reg.node_id,
                    "name": name,
                    "address": e.reg.address,
                    "cameo_version": e.reg.cameo_version,
                    "online": age < ONLINE_WINDOW,
                    "age_secs": age.as_secs(),
                    "gpus": gpu_summary(&e.reg.node),
                    "endpoints": e.reg.node.get("endpoints").cloned().unwrap_or_else(|| json!([])),
                })
            })
            .collect()
    }
}

/// Choose a node's stable id: an explicit `node_id`, else its `name`, else its
/// `address`, else a last-resort constant. Trims so whitespace never becomes an id.
fn pick_id(reg: &Registration) -> String {
    for candidate in [
        reg.node_id.as_str(),
        reg.name.as_str(),
        reg.address.as_str(),
    ] {
        let candidate = candidate.trim();
        if !candidate.is_empty() {
            return candidate.to_string();
        }
    }
    "node".to_string()
}

/// Drop nodes that have been silent past [`DROP_AFTER`].
fn prune(map: &mut BTreeMap<String, Enrolled>, now: Instant) {
    map.retain(|_, e| now.duration_since(e.last_seen) < DROP_AFTER);
}

/// A compact `[{model, vram_mb, tier}]` view of a node's GPUs, read out of its
/// stored `/api/node` body. Missing/parsed-away fields degrade to nulls rather
/// than erroring — the dashboard renders whatever the node reported.
fn gpu_summary(node: &Value) -> Value {
    match node.get("gpus").and_then(Value::as_array) {
        Some(list) => Value::Array(
            list.iter()
                .map(|a| {
                    let gpu = a.get("gpu");
                    json!({
                        "model": gpu.and_then(|g| g.get("model")).cloned()
                            .unwrap_or_else(|| json!("GPU")),
                        "vram_mb": gpu.and_then(|g| g.get("vram_mb")).cloned()
                            .unwrap_or(Value::Null),
                        "tier": a.get("tier").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect(),
        ),
        None => json!([]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registration as it arrives over the wire, so serde defaults are exercised
    /// the same way `POST /hub/register` hits them.
    fn reg(v: Value) -> Registration {
        serde_json::from_value(v).expect("valid registration json")
    }

    /// A minimal `/api/node`-shaped description with one GPU.
    fn node_desc() -> Value {
        json!({
            "name": "box-a",
            "gpus": [{
                "gpu": { "model": "Radeon RX 7900 XTX", "vram_mb": 24560 },
                "tier": "Tier1"
            }],
            "endpoints": [{ "id": "tiny-8080", "model": "tinyllama" }]
        })
    }

    #[test]
    fn register_derives_a_stable_id_and_is_idempotent() {
        let farm = Farm::new();
        // No node_id: falls back to the name.
        let id = farm.register(reg(json!({ "name": "box-a", "address": "10.0.0.2:9090" })));
        assert_eq!(id, "box-a");
        // Same node again updates in place, not a second row.
        farm.register(reg(json!({ "name": "box-a", "address": "10.0.0.2:9090" })));
        assert_eq!(farm.list().len(), 1);
    }

    #[test]
    fn id_falls_back_to_address_then_constant() {
        let farm = Farm::new();
        assert_eq!(
            farm.register(reg(json!({ "address": "10.0.0.9:9090" }))),
            "10.0.0.9:9090"
        );
        // Nothing usable at all still yields a row rather than panicking.
        assert_eq!(farm.register(reg(json!({ "address": "" }))), "node");
    }

    #[test]
    fn a_fresh_node_lists_online_with_its_gpu_summary() {
        let farm = Farm::new();
        farm.register(reg(json!({
            "name": "box-a",
            "address": "10.0.0.2:9090",
            "cameo_version": "0.1.0",
            "node": node_desc()
        })));
        let list = farm.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["online"], json!(true), "just-registered is online");
        assert_eq!(list[0]["gpus"][0]["model"], "Radeon RX 7900 XTX");
        assert_eq!(list[0]["gpus"][0]["vram_mb"], 24560);
        assert_eq!(list[0]["gpus"][0]["tier"], "Tier1");
        assert_eq!(list[0]["endpoints"][0]["model"], "tinyllama");
    }

    #[test]
    fn heartbeat_refreshes_known_and_rejects_unknown() {
        let farm = Farm::new();
        let id = farm.register(reg(json!({ "name": "box-a", "address": "10.0.0.2:9090" })));
        assert!(farm.heartbeat(&id, None), "known node accepts a beat");
        assert!(
            !farm.heartbeat("ghost", None),
            "an unknown node is rejected so its agent re-registers"
        );
    }

    #[test]
    fn heartbeat_updates_the_live_description() {
        let farm = Farm::new();
        let id = farm.register(reg(json!({ "name": "box-a", "address": "10.0.0.2:9090" })));
        assert!(farm.heartbeat(&id, Some(node_desc())));
        // The beat's description is now what the dashboard shows.
        assert_eq!(farm.list()[0]["gpus"][0]["tier"], "Tier1");
        // A null description on a beat leaves the last-known one intact.
        assert!(farm.heartbeat(&id, Some(Value::Null)));
        assert_eq!(farm.list()[0]["gpus"][0]["tier"], "Tier1");
    }

    #[test]
    fn push_target_returns_address_and_key() {
        let farm = Farm::new();
        let id = farm.register(reg(json!({
            "name": "box-a", "address": "10.0.0.2:9090", "key": "sekret"
        })));
        assert_eq!(
            farm.push_target(&id),
            Some(("10.0.0.2:9090".to_string(), Some("sekret".to_string())))
        );
        assert_eq!(farm.push_target("ghost"), None);
    }

    #[test]
    fn remove_reports_whether_it_deleted() {
        let farm = Farm::new();
        let id = farm.register(reg(json!({ "name": "box-a", "address": "10.0.0.2:9090" })));
        assert!(farm.remove(&id));
        assert!(!farm.remove(&id));
        assert!(farm.list().is_empty());
    }
}
