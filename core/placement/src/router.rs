//! Live-usage fleet routing: delegate a task to the best node by **usage**,
//! **card**, and **model**.
//!
//! [`crate::fleet::place_on_fleet`] already routes by card (tier eligibility) and
//! model (VRAM fit), but it reasons from a node's *static* topology — it does not
//! know what a node is *currently running*. This module adds the missing axis:
//! given each candidate node's **live load** (which models it already serves and
//! how much VRAM is committed), it prefers
//!
//! 1. a node **already serving the model** (warm — no reload, no extra VRAM), then
//! 2. the **least-loaded** eligible node (most free VRAM, then fewest endpoints),
//!
//! among nodes that satisfy the **card** constraint and can hold the model. It is
//! the brain a harness (Knossos) calls to delegate work across your boxes; it is
//! pure and deterministic, so it is fully unit-tested off-hardware.

use serde::Serialize;

use crate::fleet::NodeInfo;
use crate::model::ModelMeta;
use crate::plan::{Task, TRAINING_FOOTPRINT_MULT};

/// A node's live load, as the hub knows it from heartbeats.
#[derive(Debug, Clone, Default)]
pub struct NodeLoad {
    /// Model names this node is currently serving (a match means "warm").
    pub serving: Vec<String>,
    /// VRAM already committed to running endpoints, best-effort. `0` means idle
    /// or unknown — treated as no commitment.
    pub used_vram_bytes: u64,
}

impl NodeLoad {
    fn serves(&self, model: &str) -> bool {
        self.serving.iter().any(|m| m == model)
    }
}

/// A node the router may place work on: its static description plus its live load.
pub struct Candidate<'a> {
    pub node: &'a NodeInfo,
    pub load: NodeLoad,
}

/// What to place, and the card constraint to honor.
#[derive(Debug, Clone)]
pub struct RouteRequest {
    pub model: ModelMeta,
    pub task: Task,
    /// Minimum GPU tier the task needs: `Some(1)` requires a Tier-1 node, `Some(2)`
    /// allows Tier 1 or 2, `None` allows any card (and even a CPU-only node for
    /// inference). Training always additionally requires a training-capable node.
    pub min_tier: Option<u8>,
}

/// The routing decision: which candidate won, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RouteChoice {
    /// Index into the candidate slice handed to [`route`].
    pub index: usize,
    pub node_name: String,
    /// True when the node already serves the model — delegate without a reload.
    pub warm: bool,
    /// Free VRAM on the chosen node after its current load (`0` if unknown).
    pub free_vram_bytes: u64,
    pub reason: String,
}

/// Why routing could not choose a node.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteError {
    /// No nodes were offered at all.
    NoCandidates,
    /// Nodes exist, but none satisfy the card constraint and can hold the model.
    NoneEligible(String),
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteError::NoCandidates => write!(f, "no nodes available to route to"),
            RouteError::NoneEligible(why) => write!(f, "no eligible node: {why}"),
        }
    }
}

impl std::error::Error for RouteError {}

/// Free VRAM on a candidate after its current load, and whether VRAM is known.
fn free_vram(c: &Candidate) -> (u64, bool) {
    let (usable, known) = c.node.usable_vram();
    (usable.saturating_sub(c.load.used_vram_bytes), known)
}

/// Whether a candidate satisfies the card (tier) constraint for the task.
fn card_ok(c: &Candidate, req: &RouteRequest) -> bool {
    if matches!(req.task, Task::Training) && !c.node.training_capable() {
        return false;
    }
    match req.min_tier {
        // A lower tier *number* is a better card; "meets min_tier t" means the
        // node's top card is tier `t` or better.
        Some(t) => c
            .node
            .assessments
            .first()
            .map(|a| a.tier.as_number() <= t)
            .unwrap_or(false),
        None => true,
    }
}

/// A candidate's comparable rank. Ordering encodes the policy: model (warm) beats
/// usage (free VRAM) beats usage (endpoint count).
struct Ranked {
    warm: bool,
    free: u64,
    endpoints: usize,
}

impl Ranked {
    fn better_than(&self, other: &Ranked) -> bool {
        if self.warm != other.warm {
            return self.warm; // a warm node always wins
        }
        if self.free != other.free {
            return self.free > other.free; // least loaded = most free VRAM
        }
        self.endpoints < other.endpoints // tiebreak: fewer running endpoints
    }
}

/// Pick the best node to delegate a task to, by usage → card → model.
///
/// Eligibility: the node must satisfy the card constraint **and** either already
/// serve the model (warm) or have room for it. When a node's VRAM is unknown, it
/// is treated as roomy rather than excluded — the router advises, and a later
/// admission check on the node itself is the true gate.
pub fn route(candidates: &[Candidate], req: &RouteRequest) -> Result<RouteChoice, RouteError> {
    if candidates.is_empty() {
        return Err(RouteError::NoCandidates);
    }

    let need = match req.task {
        Task::Inference => req.model.total_bytes(),
        Task::Training => req.model.weights_bytes() * TRAINING_FOOTPRINT_MULT,
    };

    let mut carded = 0usize;
    let mut best: Option<(usize, Ranked)> = None;
    for (i, c) in candidates.iter().enumerate() {
        if !card_ok(c, req) {
            continue;
        }
        carded += 1;
        let warm = c.load.serves(&req.model.name);
        let (free, known) = free_vram(c);
        // Fits if warm (already resident), or the model fits free VRAM, or VRAM is
        // unknown (advise and let the node's own admission be the real gate).
        if !(warm || !known || need <= free) {
            continue;
        }
        let ranked = Ranked {
            warm,
            free,
            endpoints: c.load.serving.len(),
        };
        if best.as_ref().is_none_or(|(_, b)| ranked.better_than(b)) {
            best = Some((i, ranked));
        }
    }

    match best {
        Some((i, r)) => {
            let c = &candidates[i];
            let reason = if r.warm {
                format!(
                    "'{}' already serves {} — delegating warm (no reload)",
                    c.node.name, req.model.name
                )
            } else {
                format!(
                    "'{}' is the least-loaded eligible node ({:.1} GiB free)",
                    c.node.name,
                    r.free as f64 / (1024.0 * 1024.0 * 1024.0)
                )
            };
            Ok(RouteChoice {
                index: i,
                node_name: c.node.name.clone(),
                warm: r.warm,
                free_vram_bytes: r.free,
                reason,
            })
        }
        None if carded == 0 => Err(RouteError::NoneEligible(
            "no node meets the card/tier requirement".into(),
        )),
        None => Err(RouteError::NoneEligible(format!(
            "{carded} node(s) meet the card requirement but none can hold {}",
            req.model.name
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::NodeInfo;
    use crate::model::QuantLevel;
    use cameo_gpu_detect::{classify, GpuInfo, MemoryKind, OverrideDb, Topology};

    fn node(name: &str, gfx: &str, vram_mb: u64) -> NodeInfo {
        let db = OverrideDb::embedded();
        let gpu = GpuInfo {
            model: gfx.into(),
            pci_id: "1002:0000".into(),
            vram_mb: Some(vram_mb),
            gfx_arch: Some(gfx.into()),
            memory: MemoryKind::Dedicated,
            ..Default::default()
        };
        let assessments = vec![classify(gpu.clone(), &db)];
        NodeInfo {
            name: name.into(),
            address: format!("{name}:9090"),
            topology: Topology::new(vec![gpu], Vec::new()),
            assessments,
        }
    }

    fn load(serving: &[&str], used_gib: u64) -> NodeLoad {
        NodeLoad {
            serving: serving.iter().map(|s| s.to_string()).collect(),
            used_vram_bytes: used_gib * 1024 * 1024 * 1024,
        }
    }

    fn req(model: &str, min_tier: Option<u8>) -> RouteRequest {
        RouteRequest {
            model: ModelMeta::dense(model, 7.0, QuantLevel::Q4_K_M),
            task: Task::Inference,
            min_tier,
        }
    }

    #[test]
    fn warm_node_wins_even_when_busier() {
        // 'warm' already serves the model but is loaded; 'idle' is empty. Warm wins.
        let (a, b) = (
            node("warm", "gfx1100", 24576),
            node("idle", "gfx1100", 24576),
        );
        let cands = vec![
            Candidate {
                node: &a,
                load: load(&["qwen-7b"], 20),
            },
            Candidate {
                node: &b,
                load: load(&[], 0),
            },
        ];
        let choice = route(&cands, &req("qwen-7b", None)).unwrap();
        assert_eq!(choice.node_name, "warm");
        assert!(choice.warm);
    }

    #[test]
    fn among_cold_nodes_the_least_loaded_wins() {
        // Neither serves the model; 'free' has more headroom, so it wins on usage.
        let (a, b) = (
            node("busy", "gfx1100", 24576),
            node("free", "gfx1100", 24576),
        );
        let cands = vec![
            Candidate {
                node: &a,
                load: load(&["other"], 18),
            },
            Candidate {
                node: &b,
                load: load(&["other"], 2),
            },
        ];
        let choice = route(&cands, &req("qwen-7b", None)).unwrap();
        assert_eq!(choice.node_name, "free");
        assert!(!choice.warm);
    }

    #[test]
    fn card_constraint_excludes_too_low_a_tier() {
        // gfx803 (Polaris) is Tier 3; requiring Tier 1 rules it out, leaving the 7900.
        let (old, new) = (node("old", "gfx803", 8192), node("new", "gfx1100", 24576));
        let cands = vec![
            Candidate {
                node: &old,
                load: load(&[], 0),
            },
            Candidate {
                node: &new,
                load: load(&[], 10),
            },
        ];
        let choice = route(&cands, &req("qwen-7b", Some(1))).unwrap();
        assert_eq!(choice.node_name, "new");
    }

    #[test]
    fn training_skips_non_training_capable_nodes() {
        let (old, new) = (node("old", "gfx803", 8192), node("new", "gfx1100", 24576));
        let cands = vec![
            Candidate {
                node: &old,
                load: load(&[], 0),
            },
            Candidate {
                node: &new,
                load: load(&[], 0),
            },
        ];
        let r = RouteRequest {
            model: ModelMeta::dense("qwen-7b", 7.0, QuantLevel::Q4_K_M),
            task: Task::Training,
            min_tier: None,
        };
        assert_eq!(route(&cands, &r).unwrap().node_name, "new");
    }

    #[test]
    fn nothing_fits_reports_none_eligible() {
        // A tiny 4 GiB node, a model far larger than it, and no warm copy anywhere.
        let small = node("small", "gfx1100", 4096);
        let cands = vec![Candidate {
            node: &small,
            load: load(&[], 3),
        }];
        let r = RouteRequest {
            model: ModelMeta::dense("llama-70b", 70.0, QuantLevel::Q4_K_M),
            task: Task::Inference,
            min_tier: None,
        };
        assert!(matches!(
            route(&cands, &r),
            Err(RouteError::NoneEligible(_))
        ));
    }

    #[test]
    fn no_candidates_is_distinct_from_none_eligible() {
        assert_eq!(route(&[], &req("x", None)), Err(RouteError::NoCandidates));
    }
}
