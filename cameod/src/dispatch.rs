//! Hub-side task dispatch: turn the live fleet roster into a routing decision.
//!
//! This is the harness-facing "delegate this task to a box" brain. It reconstructs
//! each online node from the `/api/node` description it phoned home with, reads its
//! live load out of its running endpoints, and asks [`cameo_placement::route`] to
//! pick the best node by usage → card → model. The routing is pure and unit-tested
//! against a canned roster; the actual serve (when `execute` is set) is the hub's
//! existing push to the node's `/api/servers`, done in [`crate::app`].

use cameo_gpu_detect::{TierAssessment, Topology};
use cameo_placement::{
    route_mesh, MeshCandidate, MeshHealth, MeshPreference, MeshPrivacy, MeshRequest,
    MeshRouteChoice, ModelMeta, NodeInfo, NodeLoad, QuantLevel, RouteError, RouteRequest, Task,
    TrustState,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const ADMISSION_TTL: Duration = Duration::from_secs(45);
const MAX_ADMISSIONS: usize = 4096;

/// The dispatch request a harness POSTs to `/hub/dispatch`.
#[derive(Clone, Deserialize, Serialize)]
pub struct DispatchBody {
    pub model: String,
    #[serde(default)]
    pub params: Option<f64>,
    #[serde(default = "default_quant")]
    pub quant: String,
    #[serde(default)]
    pub moe: bool,
    /// `"inference"` (default) or `"training"`.
    #[serde(default)]
    pub task: Option<String>,
    /// Minimum GPU tier: `1` requires Tier 1, `2` allows Tier 1/2, absent = any.
    #[serde(default)]
    pub min_tier: Option<u8>,
    /// When true, serve the model on the chosen node; otherwise just advise.
    #[serde(default)]
    pub execute: bool,
    /// Port to serve on when executing.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Keep a session on its prior node when that node still passes every hard
    /// admission gate. This is an operator-authenticated hint, not an override.
    #[serde(default)]
    pub session_affinity: Option<String>,
    #[serde(default)]
    pub privacy: MeshPrivacy,
    #[serde(default)]
    pub preference: MeshPreference,
    /// End-to-end completion target. Nodes predicted to miss it are excluded.
    #[serde(default)]
    pub deadline_ms: Option<u64>,
    #[serde(default = "default_expected_output_tokens")]
    pub expected_output_tokens: u32,
    /// Reserved for admission-queue ordering. Bounded now so the wire contract
    /// cannot later acquire unbounded or wraparound semantics.
    #[serde(default = "default_priority")]
    pub priority: u8,
    /// Compatibility switch for the current shared farm-token enrollment. Set
    /// false to require device-bound pairing once paired nodes are enrolled.
    #[serde(default = "default_true")]
    pub allow_legacy_token: bool,
    #[serde(default = "default_protocol_major")]
    pub protocol_major: u16,
    /// Stable caller identity for execute retries. Omit for legacy at-most-once
    /// callers; new harnesses should always send one.
    #[serde(default)]
    pub request_id: Option<String>,
}

fn default_quant() -> String {
    "Q4_K_M".into()
}
fn default_port() -> u16 {
    8080
}
fn default_expected_output_tokens() -> u32 {
    512
}
fn default_priority() -> u8 {
    5
}
fn default_true() -> bool {
    true
}
fn default_protocol_major() -> u16 {
    1
}

impl DispatchBody {
    /// Reject ambiguous or resource-amplifying inputs before looking at the
    /// roster. The HTTP layer maps this to 400 rather than pretending a bad
    /// request is a temporarily full fleet.
    pub fn validate(&self) -> Result<(), String> {
        if self.model.trim().is_empty() || self.model.len() > 512 {
            return Err("model must be 1..=512 bytes".into());
        }
        if QuantLevel::parse(&self.quant).is_none() {
            return Err(format!("unsupported quant '{}'", self.quant));
        }
        if !matches!(
            self.task.as_deref(),
            None | Some("inference" | "training" | "train")
        ) {
            return Err("task must be inference or training".into());
        }
        if self.min_tier.is_some_and(|tier| !(1..=3).contains(&tier)) {
            return Err("min_tier must be 1, 2, or 3".into());
        }
        if self.execute && self.port == 0 {
            return Err("port must be non-zero when execute is true".into());
        }
        if self.priority > 9 {
            return Err("priority must be between 0 and 9".into());
        }
        if self.expected_output_tokens == 0 || self.expected_output_tokens > 1_000_000 {
            return Err("expected_output_tokens must be between 1 and 1000000".into());
        }
        if self.deadline_ms == Some(0) {
            return Err("deadline_ms must be positive".into());
        }
        if self.protocol_major == 0 {
            return Err("protocol_major must be positive".into());
        }
        if self
            .session_affinity
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 256)
        {
            return Err("session_affinity must be 1..=256 bytes when present".into());
        }
        if self
            .request_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 128)
        {
            return Err("request_id must be 1..=128 bytes when present".into());
        }
        Ok(())
    }

    fn model_meta(&self) -> ModelMeta {
        let quant = QuantLevel::parse(&self.quant).unwrap_or(QuantLevel::Q4_K_M);
        let params = self
            .params
            .or_else(|| cameo_models::params_b_for(&self.model))
            .unwrap_or(7.0);
        if self.moe {
            ModelMeta::moe(&self.model, params, quant)
        } else {
            ModelMeta::dense(&self.model, params, quant)
        }
    }

    fn task(&self) -> Task {
        match self.task.as_deref() {
            Some("training") | Some("train") => Task::Training,
            _ => Task::Inference,
        }
    }

    fn route_request(&self) -> RouteRequest {
        RouteRequest {
            model: self.model_meta(),
            task: self.task(),
            min_tier: self.min_tier,
        }
    }

    fn mesh_request(&self) -> MeshRequest {
        MeshRequest {
            placement: self.route_request(),
            session_affinity: self.session_affinity.clone(),
            privacy: self.privacy,
            preference: self.preference,
            deadline_ms: self.deadline_ms,
            expected_output_tokens: self.expected_output_tokens,
            priority: self.priority,
            allow_legacy_token: self.allow_legacy_token,
            protocol_major: self.protocol_major,
        }
    }
}

/// A node reconstructed from its stored description, kept alongside the `node_id`
/// the hub pushes work back to.
struct ParsedNode {
    node_id: String,
    info: NodeInfo,
    load: NodeLoad,
    health: MeshHealth,
    protocol_major: u16,
    owner_reclaim: bool,
    inflight: u32,
    estimated_ttft_ms: u64,
    tokens_per_second: f64,
    trust: TrustState,
}

/// Reconstruct a routable node from its stored `/api/node` body, or `None` if it
/// lacks the topology/assessments the router needs (e.g. a dev node that enrolled
/// without detection).
fn parse_node(node_id: &str, desc: &Value) -> Option<ParsedNode> {
    let topology: Topology = serde_json::from_value(desc.get("topology")?.clone()).ok()?;
    let assessments: Vec<TierAssessment> =
        serde_json::from_value(desc.get("gpus")?.clone()).ok()?;
    let name = desc
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(node_id)
        .to_string();
    let load = load_from_endpoints(desc.get("endpoints"));
    let mesh = desc.get("mesh");
    let health = match mesh.and_then(|m| m.get("health")).and_then(Value::as_str) {
        Some("degraded") => MeshHealth::Degraded,
        Some("draining") => MeshHealth::Draining,
        Some("offline") => MeshHealth::Offline,
        _ => MeshHealth::Ready,
    };
    Some(ParsedNode {
        node_id: node_id.to_string(),
        info: NodeInfo {
            name,
            address: String::new(), // not needed for routing; push uses node_id
            topology,
            assessments,
            resident: load.serving.clone(),
        },
        load,
        health,
        protocol_major: mesh
            .and_then(|m| m.get("protocol_major"))
            .and_then(Value::as_u64)
            .and_then(|v| u16::try_from(v).ok())
            .unwrap_or(1),
        owner_reclaim: mesh
            .and_then(|m| m.get("owner_reclaim"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        inflight: mesh
            .and_then(|m| m.get("inflight"))
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0),
        estimated_ttft_ms: mesh
            .and_then(|m| m.get("estimated_ttft_ms"))
            .and_then(Value::as_u64)
            .unwrap_or(750),
        tokens_per_second: mesh
            .and_then(|m| m.get("tokens_per_second"))
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(20.0),
        trust: match desc
            .get("_cameo_hub")
            .and_then(|hub| hub.get("trust"))
            .and_then(Value::as_str)
        {
            Some("paired") => TrustState::Paired,
            Some("untrusted") => TrustState::Untrusted,
            _ => TrustState::LegacyToken,
        },
    })
}

/// A node's live load from its `endpoints` array: the models it is *running* and
/// the VRAM those hold. Non-running endpoints (exited/failed) hold nothing.
fn load_from_endpoints(endpoints: Option<&Value>) -> NodeLoad {
    let mut serving = Vec::new();
    let mut used = 0u64;
    if let Some(arr) = endpoints.and_then(Value::as_array) {
        for e in arr {
            if e.get("state").and_then(Value::as_str) != Some("running") {
                continue;
            }
            if let Some(m) = e.get("model").and_then(Value::as_str) {
                serving.push(m.to_string());
            }
            used = used.saturating_add(e.get("vram_bytes").and_then(Value::as_u64).unwrap_or(0));
        }
    }
    NodeLoad {
        serving,
        used_vram_bytes: used,
    }
}

/// A routing decision plus the `node_id` to push the serve to.
#[derive(Clone)]
pub struct Dispatch {
    pub choice: MeshRouteChoice,
    pub node_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionState {
    Reserved,
    Committed,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdmissionLease {
    pub id: String,
    pub request_id: String,
    pub node_id: String,
    pub model: String,
    pub reserved_vram_bytes: u64,
    pub expires_in_ms: u64,
    pub state: AdmissionState,
}

pub struct AdmissionOutcome {
    pub dispatch: Dispatch,
    pub lease: Option<AdmissionLease>,
    pub replayed: bool,
}

struct Reservation {
    fingerprint: String,
    dispatch: Dispatch,
    lease: AdmissionLease,
    expires: Instant,
}

struct AdmissionStateMap {
    by_request: BTreeMap<String, Reservation>,
}

/// Atomic in-process admission book. It closes the concurrent overbooking gap
/// between hub selection and the node's authoritative local admission check.
pub struct AdmissionBook {
    inner: Mutex<AdmissionStateMap>,
    next_id: AtomicU64,
}

impl Default for AdmissionBook {
    fn default() -> Self {
        Self::new()
    }
}

impl AdmissionBook {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(AdmissionStateMap {
                by_request: BTreeMap::new(),
            }),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn admit(
        &self,
        roster: &[(String, Value)],
        body: &DispatchBody,
    ) -> Result<AdmissionOutcome, AdmissionError> {
        let fingerprint = serde_json::to_string(body)
            .map_err(|e| AdmissionError::Invalid(format!("cannot identify request: {e}")))?;
        let now = Instant::now();
        let mut state = self.inner.lock().unwrap();
        state.by_request.retain(|_, item| item.expires > now);

        if let Some(request_id) = body.request_id.as_deref() {
            if let Some(existing) = state.by_request.get(request_id) {
                if existing.fingerprint != fingerprint {
                    return Err(AdmissionError::Conflict(format!(
                        "request_id '{request_id}' was already used with a different dispatch body"
                    )));
                }
                return Ok(AdmissionOutcome {
                    dispatch: existing.dispatch.clone(),
                    lease: Some(existing.lease.clone()),
                    replayed: true,
                });
            }
        }

        let reserved = active_reserved_by_node(&state.by_request);
        let dispatch = decide_with_reserved(roster, body, &reserved)?;
        if !body.execute {
            return Ok(AdmissionOutcome {
                dispatch,
                lease: None,
                replayed: false,
            });
        }
        if state.by_request.len() >= MAX_ADMISSIONS {
            return Err(AdmissionError::Capacity(
                "admission ledger is full; retry after a lease expires".into(),
            ));
        }

        let serial = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request_id = body
            .request_id
            .clone()
            .unwrap_or_else(|| format!("legacy-{serial}"));
        let id = format!("adm-{serial}");
        let reserved_vram_bytes = if dispatch.choice.warm {
            0
        } else {
            body.model_meta().total_bytes()
        };
        let lease = AdmissionLease {
            id,
            request_id: request_id.clone(),
            node_id: dispatch.node_id.clone(),
            model: body.model.clone(),
            reserved_vram_bytes,
            expires_in_ms: ADMISSION_TTL.as_millis() as u64,
            state: AdmissionState::Reserved,
        };
        state.by_request.insert(
            request_id,
            Reservation {
                fingerprint,
                dispatch: dispatch.clone(),
                lease: lease.clone(),
                expires: now + ADMISSION_TTL,
            },
        );
        Ok(AdmissionOutcome {
            dispatch,
            lease: Some(lease),
            replayed: false,
        })
    }

    /// Mark the remote node result without deleting capacity protection. A
    /// committed lease remains until heartbeat telemetry has had time to catch
    /// up; a failed lease stops counting immediately but remains idempotent.
    pub fn finish(&self, admission_id: &str, success: bool) {
        let mut state = self.inner.lock().unwrap();
        if let Some(item) = state
            .by_request
            .values_mut()
            .find(|item| item.lease.id == admission_id)
        {
            item.lease.state = if success {
                AdmissionState::Committed
            } else {
                AdmissionState::Failed
            };
        }
    }
}

#[derive(Debug)]
pub enum AdmissionError {
    Route(RouteError),
    Invalid(String),
    Conflict(String),
    Capacity(String),
}

impl AdmissionError {
    pub fn status(&self) -> u16 {
        match self {
            Self::Invalid(_) => 400,
            Self::Conflict(_) | Self::Route(_) => 409,
            Self::Capacity(_) => 429,
        }
    }
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Route(e) => e.fmt(f),
            Self::Invalid(e) | Self::Conflict(e) | Self::Capacity(e) => f.write_str(e),
        }
    }
}

impl From<RouteError> for AdmissionError {
    fn from(value: RouteError) -> Self {
        Self::Route(value)
    }
}

fn active_reserved_by_node(reservations: &BTreeMap<String, Reservation>) -> BTreeMap<String, u64> {
    let mut by_node = BTreeMap::new();
    for item in reservations.values().filter(|item| {
        matches!(
            item.lease.state,
            AdmissionState::Reserved | AdmissionState::Committed
        )
    }) {
        let entry = by_node.entry(item.lease.node_id.clone()).or_insert(0u64);
        *entry = entry.saturating_add(item.lease.reserved_vram_bytes);
    }
    by_node
}

/// Decide where a task should run, given the raw online roster
/// (`(node_id, /api/node description)` pairs) and the request.
#[cfg(test)]
pub fn decide(roster: &[(String, Value)], body: &DispatchBody) -> Result<Dispatch, RouteError> {
    decide_with_reserved(roster, body, &BTreeMap::new())
}

fn decide_with_reserved(
    roster: &[(String, Value)],
    body: &DispatchBody,
    reserved_by_node: &BTreeMap<String, u64>,
) -> Result<Dispatch, RouteError> {
    let mut parsed: Vec<ParsedNode> = roster
        .iter()
        .filter_map(|(id, desc)| parse_node(id, desc))
        .collect();
    for node in &mut parsed {
        node.load.used_vram_bytes = node
            .load
            .used_vram_bytes
            .saturating_add(reserved_by_node.get(&node.node_id).copied().unwrap_or(0));
    }
    let candidates: Vec<MeshCandidate> = parsed
        .iter()
        .map(|p| MeshCandidate {
            node_id: &p.node_id,
            node: &p.info,
            load: p.load.clone(),
            // A farm dispatch crosses a network boundary. Until Cameo has a
            // verified same-device transport, local-only requests fail closed.
            local: false,
            // `_cameo_hub.trust` is overwritten by Farm when it creates the
            // roster, so node-supplied telemetry cannot promote itself.
            trust: p.trust,
            health: p.health,
            protocol_major: p.protocol_major,
            owner_reclaim: p.owner_reclaim,
            inflight: p.inflight,
            estimated_ttft_ms: p.estimated_ttft_ms,
            tokens_per_second: p.tokens_per_second,
        })
        .collect();
    let choice = route_mesh(&candidates, &body.mesh_request())?;
    Ok(Dispatch {
        node_id: choice.node_id.clone(),
        choice,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A roster row shaped like what a node phones home with (`/api/node` body),
    /// with `vram_mb` on its GPU and any running endpoints.
    fn node_desc(name: &str, gfx: &str, vram_mb: u64, serving: &[(&str, u64)]) -> Value {
        let endpoints: Vec<Value> = serving
            .iter()
            .map(|(m, gb)| json!({ "model": m, "state": "running", "vram_bytes": gb * 1024*1024*1024 }))
            .collect();
        json!({
            "name": name,
            "topology": {
                "gpus": [{
                    "model": gfx, "vendor": "amd", "pci_id": "1002:0000",
                    "vram_mb": vram_mb, "memory": "dedicated", "gfx_arch": gfx
                }],
                "links": [],
                "host_mem": { "total_bytes": 34359738368u64, "available_bytes": 20000000000u64 }
            },
            "gpus": [{
                "gpu": {
                    "model": gfx, "vendor": "amd", "pci_id": "1002:0000",
                    "vram_mb": vram_mb, "memory": "dedicated", "gfx_arch": gfx
                },
                "tier": "Tier1", "training_supported": true, "rationale": "test"
            }],
            "endpoints": endpoints
        })
    }

    fn body(model: &str, execute: bool) -> DispatchBody {
        DispatchBody {
            model: model.into(),
            params: Some(7.0),
            quant: "Q4_K_M".into(),
            moe: false,
            task: None,
            min_tier: None,
            execute,
            port: 8080,
            session_affinity: None,
            privacy: MeshPrivacy::Pool,
            preference: MeshPreference::Balanced,
            deadline_ms: None,
            expected_output_tokens: 512,
            priority: 5,
            allow_legacy_token: true,
            protocol_major: 1,
            request_id: None,
        }
    }

    #[test]
    fn dispatch_prefers_the_warm_node() {
        let roster = vec![
            (
                "a".into(),
                node_desc("a", "gfx1100", 24576, &[("qwen-7b", 5)]),
            ),
            ("b".into(), node_desc("b", "gfx1100", 24576, &[])),
        ];
        let d = decide(&roster, &body("qwen-7b", false)).unwrap();
        assert_eq!(d.node_id, "a");
        assert!(
            d.choice.warm,
            "the node already serving the model is chosen warm"
        );
    }

    #[test]
    fn dispatch_spreads_to_the_least_loaded_when_cold() {
        // Neither serves qwen; 'b' holds less VRAM, so usage routes there.
        let roster = vec![
            (
                "a".into(),
                node_desc("a", "gfx1100", 24576, &[("other", 18)]),
            ),
            (
                "b".into(),
                node_desc("b", "gfx1100", 24576, &[("other", 2)]),
            ),
        ];
        let d = decide(&roster, &body("qwen-7b", false)).unwrap();
        assert_eq!(d.node_id, "b");
        assert!(!d.choice.warm);
    }

    #[test]
    fn dispatch_errors_when_the_roster_is_empty() {
        assert!(matches!(
            decide(&[], &body("qwen-7b", false)),
            Err(RouteError::NoCandidates)
        ));
    }

    #[test]
    fn a_description_without_topology_is_skipped_not_fatal() {
        // One malformed row + one good row: routing still succeeds on the good one.
        let roster = vec![
            ("bad".into(), json!({ "name": "bad" })),
            ("good".into(), node_desc("good", "gfx1100", 24576, &[])),
        ];
        let d = decide(&roster, &body("qwen-7b", false)).unwrap();
        assert_eq!(d.node_id, "good");
    }

    #[test]
    fn local_only_fails_closed_for_network_farm_dispatch() {
        let roster = vec![("remote".into(), node_desc("remote", "gfx1100", 24576, &[]))];
        let mut request = body("qwen-7b", false);
        request.privacy = MeshPrivacy::LocalOnly;
        assert!(matches!(
            decide(&roster, &request),
            Err(RouteError::NoneEligible(_))
        ));
    }

    #[test]
    fn strict_pairing_rejects_legacy_farm_nodes() {
        let roster = vec![("legacy".into(), node_desc("legacy", "gfx1100", 24576, &[]))];
        let mut request = body("qwen-7b", false);
        request.allow_legacy_token = false;
        assert!(matches!(
            decide(&roster, &request),
            Err(RouteError::NoneEligible(_))
        ));
    }

    #[test]
    fn malformed_dispatch_limits_are_rejected() {
        let mut request = body("qwen-7b", false);
        request.priority = 10;
        assert!(request.validate().is_err());
        request.priority = 5;
        request.expected_output_tokens = 0;
        assert!(request.validate().is_err());
    }

    #[test]
    fn execute_request_is_idempotent_and_conflicting_reuse_is_rejected() {
        let roster = vec![("a".into(), node_desc("a", "gfx1100", 24576, &[]))];
        let book = AdmissionBook::new();
        let mut request = body("qwen-7b", true);
        request.request_id = Some("req-1".into());
        let first = book.admit(&roster, &request).unwrap();
        let replay = book.admit(&roster, &request).unwrap();
        assert!(!first.replayed);
        assert!(replay.replayed);
        assert_eq!(
            first.lease.as_ref().unwrap().id,
            replay.lease.as_ref().unwrap().id
        );

        let mut conflict = request;
        conflict.model = "other".into();
        assert!(matches!(
            book.admit(&roster, &conflict),
            Err(AdmissionError::Conflict(_))
        ));
    }

    #[test]
    fn active_reservation_prevents_concurrent_overbooking() {
        // A 7B Q4 model is ~4.2 GiB. With only 8 GiB raw / 7.2 GiB usable,
        // the second distinct request must not reserve the same headroom.
        let roster = vec![("a".into(), node_desc("a", "gfx1100", 8192, &[]))];
        let book = AdmissionBook::new();
        let mut first = body("qwen-7b", true);
        first.request_id = Some("req-a".into());
        book.admit(&roster, &first).unwrap();
        let mut second = first.clone();
        second.request_id = Some("req-b".into());
        assert!(matches!(
            book.admit(&roster, &second),
            Err(AdmissionError::Route(RouteError::NoneEligible(_)))
        ));
    }

    #[test]
    fn failed_reservation_releases_capacity_but_keeps_idempotent_failure() {
        let roster = vec![("a".into(), node_desc("a", "gfx1100", 8192, &[]))];
        let book = AdmissionBook::new();
        let mut first = body("qwen-7b", true);
        first.request_id = Some("req-a".into());
        let admission = book.admit(&roster, &first).unwrap();
        book.finish(&admission.lease.as_ref().unwrap().id, false);

        let replay = book.admit(&roster, &first).unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.lease.unwrap().state, AdmissionState::Failed);

        let mut second = first;
        second.request_id = Some("req-b".into());
        assert!(book.admit(&roster, &second).is_ok());
    }
}
