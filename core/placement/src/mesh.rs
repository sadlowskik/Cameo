//! Trust- and latency-aware admission for a Cameo inference mesh.
//!
//! The older [`crate::router`] answers "which online card can hold this model?".
//! This layer answers the product-level question first: "which node may receive
//! this session under its trust, privacy, health, protocol, deadline, and owner
//! reclaim constraints?" The result is still advisory; the selected node must
//! perform a fresh local admission check before loading or reusing a model.

use serde::{Deserialize, Serialize};

use crate::plan::{Task, TRAINING_FOOTPRINT_MULT};
use crate::router::{card_ok, free_vram, Candidate, NodeLoad, RouteError, RouteRequest};
use crate::NodeInfo;

/// Enrollment strength asserted by the hub for a candidate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustState {
    /// Mutually authenticated, device-bound enrollment.
    Paired,
    /// Shared farm-token enrollment. Compatible with the current transport, but
    /// deliberately named so callers cannot mistake it for device identity.
    #[default]
    LegacyToken,
    /// No accepted identity proof. Never eligible.
    Untrusted,
}

/// Whether a node should accept new work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshHealth {
    #[default]
    Ready,
    Degraded,
    Draining,
    Offline,
}

/// Where request data is allowed to execute.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshPrivacy {
    #[default]
    Pool,
    LocalOnly,
}

/// Scheduling objective after hard admission constraints are satisfied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshPreference {
    Latency,
    #[default]
    Balanced,
    Throughput,
}

/// Live scheduling facts for one node. Identity and liveness must come from the
/// hub, not from an inference request.
pub struct MeshCandidate<'a> {
    pub node_id: &'a str,
    pub node: &'a NodeInfo,
    pub load: NodeLoad,
    pub local: bool,
    pub trust: TrustState,
    pub health: MeshHealth,
    pub protocol_major: u16,
    pub owner_reclaim: bool,
    pub inflight: u32,
    pub estimated_ttft_ms: u64,
    pub tokens_per_second: f64,
}

/// Request-scoped policy. `session_affinity` is advisory and is honored only
/// after every hard admission constraint is checked.
pub struct MeshRequest {
    pub placement: RouteRequest,
    pub session_affinity: Option<String>,
    pub privacy: MeshPrivacy,
    pub preference: MeshPreference,
    pub deadline_ms: Option<u64>,
    pub expected_output_tokens: u32,
    pub priority: u8,
    pub allow_legacy_token: bool,
    pub protocol_major: u16,
}

/// Explainable result returned by the mesh scheduler.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeshRouteChoice {
    /// Index into the candidate slice passed to [`route_mesh`].
    pub index: usize,
    pub node_id: String,
    pub node_name: String,
    pub warm: bool,
    pub affinity: bool,
    pub free_vram_bytes: u64,
    pub predicted_completion_ms: u64,
    pub trust: TrustState,
    pub health: MeshHealth,
    pub protocol_major: u16,
    pub reason: String,
}

struct Rank<'a> {
    affinity: bool,
    warm: bool,
    ready: bool,
    predicted_ms: u64,
    tokens_per_second: f64,
    free: u64,
    inflight: u32,
    node_id: &'a str,
}

impl Rank<'_> {
    fn better_than(&self, other: &Self, preference: MeshPreference) -> bool {
        if self.affinity != other.affinity {
            return self.affinity;
        }
        if self.warm != other.warm {
            return self.warm;
        }
        if self.ready != other.ready {
            return self.ready;
        }
        match preference {
            MeshPreference::Latency | MeshPreference::Balanced
                if self.predicted_ms != other.predicted_ms =>
            {
                return self.predicted_ms < other.predicted_ms;
            }
            MeshPreference::Throughput
                if (self.tokens_per_second - other.tokens_per_second).abs() > f64::EPSILON =>
            {
                return self.tokens_per_second > other.tokens_per_second;
            }
            _ => {}
        }
        if self.free != other.free {
            return self.free > other.free;
        }
        if self.inflight != other.inflight {
            return self.inflight < other.inflight;
        }
        self.node_id < other.node_id
    }
}

fn predicted_completion_ms(candidate: &MeshCandidate<'_>, expected_tokens: u32) -> u64 {
    let rate = if candidate.tokens_per_second.is_finite() && candidate.tokens_per_second > 0.0 {
        candidate.tokens_per_second
    } else {
        20.0
    };
    let ttft = if candidate.estimated_ttft_ms == 0 {
        750
    } else {
        candidate.estimated_ttft_ms
    };
    let generation = (f64::from(expected_tokens.max(1)) / rate * 1_000.0).ceil() as u64;
    let queue = u64::from(candidate.inflight).saturating_mul(250);
    let health_penalty = if candidate.health == MeshHealth::Degraded {
        2
    } else {
        1
    };
    ttft.saturating_add(generation)
        .saturating_add(queue)
        .saturating_mul(health_penalty)
}

/// Select a node only after all hard safety and locality constraints pass.
/// Selection is deterministic, including the final node-id tie break.
pub fn route_mesh(
    candidates: &[MeshCandidate<'_>],
    req: &MeshRequest,
) -> Result<MeshRouteChoice, RouteError> {
    if candidates.is_empty() {
        return Err(RouteError::NoCandidates);
    }

    let need = match req.placement.task {
        Task::Inference => req.placement.model.total_bytes(),
        Task::Training => req
            .placement
            .model
            .weights_bytes()
            .saturating_mul(TRAINING_FOOTPRINT_MULT),
    };
    let mut rejected = 0usize;
    let mut best: Option<(usize, Rank<'_>)> = None;

    for (index, mesh) in candidates.iter().enumerate() {
        let trusted = mesh.trust == TrustState::Paired
            || (req.allow_legacy_token && mesh.trust == TrustState::LegacyToken);
        let healthy = matches!(mesh.health, MeshHealth::Ready | MeshHealth::Degraded);
        if !trusted
            || !healthy
            || mesh.owner_reclaim
            || mesh.protocol_major != req.protocol_major
            || (req.privacy == MeshPrivacy::LocalOnly && !mesh.local)
        {
            rejected += 1;
            continue;
        }

        let placement_candidate = Candidate {
            node: mesh.node,
            load: mesh.load.clone(),
        };
        if !card_ok(&placement_candidate, &req.placement) {
            rejected += 1;
            continue;
        }
        let warm = mesh.load.serves(&req.placement.model.name);
        let (free, known) = free_vram(&placement_candidate);
        if !(warm || !known || need <= free) {
            rejected += 1;
            continue;
        }

        let predicted_ms = predicted_completion_ms(mesh, req.expected_output_tokens);
        if req
            .deadline_ms
            .is_some_and(|deadline| predicted_ms > deadline)
        {
            rejected += 1;
            continue;
        }
        let affinity = req.session_affinity.as_deref() == Some(mesh.node_id);
        let rank = Rank {
            affinity,
            warm,
            ready: mesh.health == MeshHealth::Ready,
            predicted_ms,
            tokens_per_second: mesh.tokens_per_second,
            free,
            inflight: mesh.inflight,
            node_id: mesh.node_id,
        };
        if best
            .as_ref()
            .is_none_or(|(_, current)| rank.better_than(current, req.preference))
        {
            best = Some((index, rank));
        }
    }

    let Some((index, rank)) = best else {
        return Err(RouteError::NoneEligible(format!(
            "all {} node(s) were rejected by trust, health, privacy, protocol, deadline, card, or memory admission",
            rejected
        )));
    };
    let selected = &candidates[index];
    let reason = if rank.affinity {
        format!(
            "'{}' retained eligible session affinity",
            selected.node.name
        )
    } else if rank.warm {
        format!(
            "'{}' already serves the model and avoids a reload",
            selected.node.name
        )
    } else {
        format!(
            "'{}' is the best eligible {:?} target (predicted {} ms)",
            selected.node.name, req.preference, rank.predicted_ms
        )
    };

    Ok(MeshRouteChoice {
        index,
        node_id: selected.node_id.to_string(),
        node_name: selected.node.name.clone(),
        warm: rank.warm,
        affinity: rank.affinity,
        free_vram_bytes: rank.free,
        predicted_completion_ms: rank.predicted_ms,
        trust: selected.trust,
        health: selected.health,
        protocol_major: selected.protocol_major,
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelMeta;
    use crate::QuantLevel;
    use cameo_gpu_detect::{classify, GpuInfo, MemoryKind, OverrideDb, Topology};

    fn node(name: &str, vram_mb: u64) -> NodeInfo {
        let gpu = GpuInfo {
            model: "gfx1100".into(),
            pci_id: "1002:0000".into(),
            vram_mb: Some(vram_mb),
            gfx_arch: Some("gfx1100".into()),
            memory: MemoryKind::Dedicated,
            ..Default::default()
        };
        NodeInfo {
            name: name.into(),
            address: String::new(),
            topology: Topology::new(vec![gpu.clone()], Vec::new()),
            assessments: vec![classify(gpu, &OverrideDb::embedded())],
            resident: Vec::new(),
        }
    }

    fn request(model: &str) -> MeshRequest {
        MeshRequest {
            placement: RouteRequest {
                model: ModelMeta::dense(model, 7.0, QuantLevel::Q4_K_M),
                task: Task::Inference,
                min_tier: None,
            },
            session_affinity: None,
            privacy: MeshPrivacy::Pool,
            preference: MeshPreference::Balanced,
            deadline_ms: None,
            expected_output_tokens: 512,
            priority: 5,
            allow_legacy_token: true,
            protocol_major: 1,
        }
    }

    fn candidate<'a>(id: &'a str, node: &'a NodeInfo) -> MeshCandidate<'a> {
        MeshCandidate {
            node_id: id,
            node,
            load: NodeLoad::default(),
            local: false,
            trust: TrustState::Paired,
            health: MeshHealth::Ready,
            protocol_major: 1,
            owner_reclaim: false,
            inflight: 0,
            estimated_ttft_ms: 200,
            tokens_per_second: 40.0,
        }
    }

    #[test]
    fn heterogeneous_pool_prefers_predicted_completion() {
        let (slow, fast, busy) = (
            node("slow", 16_384),
            node("fast", 24_576),
            node("busy", 24_576),
        );
        let mut a = candidate("a", &slow);
        a.tokens_per_second = 15.0;
        let mut b = candidate("b", &fast);
        b.tokens_per_second = 70.0;
        let mut c = candidate("c", &busy);
        c.tokens_per_second = 90.0;
        c.inflight = 20;
        let choice = route_mesh(&[a, b, c], &request("qwen-7b")).unwrap();
        assert_eq!(choice.node_id, "b");
    }

    #[test]
    fn affinity_wins_only_while_eligible() {
        let (a_node, b_node) = (node("a", 24_576), node("b", 24_576));
        let a = candidate("a", &a_node);
        let mut b = candidate("b", &b_node);
        b.tokens_per_second = 100.0;
        let mut req = request("qwen-7b");
        req.session_affinity = Some("a".into());
        assert_eq!(route_mesh(&[a, b], &req).unwrap().node_id, "a");

        let mut reclaimed = candidate("a", &a_node);
        reclaimed.owner_reclaim = true;
        let b = candidate("b", &b_node);
        assert_eq!(route_mesh(&[reclaimed, b], &req).unwrap().node_id, "b");
    }

    #[test]
    fn local_only_never_spills_to_remote() {
        let (local_node, remote_node) = (node("local", 24_576), node("remote", 24_576));
        let mut local = candidate("local", &local_node);
        local.local = true;
        local.tokens_per_second = 10.0;
        let mut remote = candidate("remote", &remote_node);
        remote.tokens_per_second = 100.0;
        let mut req = request("qwen-7b");
        req.privacy = MeshPrivacy::LocalOnly;
        assert_eq!(route_mesh(&[remote, local], &req).unwrap().node_id, "local");
    }

    #[test]
    fn trust_protocol_health_deadline_and_reclaim_are_hard_gates() {
        let n = node("n", 24_576);
        let mut req = request("qwen-7b");
        let mut c = candidate("n", &n);
        c.trust = TrustState::Untrusted;
        assert!(route_mesh(&[c], &req).is_err());

        let mut c = candidate("n", &n);
        c.protocol_major = 2;
        assert!(route_mesh(&[c], &req).is_err());

        let mut c = candidate("n", &n);
        c.health = MeshHealth::Draining;
        assert!(route_mesh(&[c], &req).is_err());

        let mut c = candidate("n", &n);
        c.owner_reclaim = true;
        assert!(route_mesh(&[c], &req).is_err());

        let c = candidate("n", &n);
        req.deadline_ms = Some(10);
        assert!(route_mesh(&[c], &req).is_err());
    }

    #[test]
    fn legacy_transport_must_be_explicitly_allowed() {
        let n = node("n", 24_576);
        let mut c = candidate("n", &n);
        c.trust = TrustState::LegacyToken;
        let mut req = request("qwen-7b");
        req.allow_legacy_token = false;
        assert!(route_mesh(&[c], &req).is_err());
    }

    #[test]
    fn ties_are_stable_by_node_id_not_input_order() {
        let (a_node, b_node) = (node("same", 24_576), node("same", 24_576));
        let a = candidate("a", &a_node);
        let b = candidate("b", &b_node);
        assert_eq!(
            route_mesh(&[b, a], &request("qwen-7b")).unwrap().node_id,
            "a"
        );
    }
}
