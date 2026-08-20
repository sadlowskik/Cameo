//! Fleet placement: the harness managing multiple nodes.
//!
//! A box is `GPUs + links` ([`cameo_gpu_detect::Topology`]); a cluster is
//! `nodes + a network`, each node carrying its own box-topology. This module is
//! the recursion of the single-box planner up to the fleet: given a cluster and a
//! workload, decide **which node** runs it (and, per node, reuse [`crate::plan`]).
//!
//! Scope, honestly: this is **orchestration** — placing a workload on a node and
//! observing the fleet. Running *one model sharded across nodes' GPUs*
//! (distributed execution over the network) is the v2 data path handled by
//! `net-strategy`; here it is represented as a decision, not executed.

use crate::error::Error;
use crate::model::{gib, ModelMeta};
use crate::plan::{plan, PlacementPlan, Task, TRAINING_FOOTPRINT_MULT, VRAM_HEADROOM};
use cameo_config::Settings;
use cameo_gpu_detect::{TierAssessment, Topology};
use serde::Serialize;

/// The class of network connecting the nodes. Determines whether cross-node
/// distributed execution is even worth considering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkClass {
    /// InfiniBand / RDMA — distributed execution is viable.
    Infiniband,
    /// Fast datacenter Ethernet — distributed execution is viable, with care.
    FastEthernet,
    /// Consumer/home networking — cross-node model sharding is bandwidth-bound.
    Consumer,
}

impl NetworkClass {
    /// Whether cross-node distributed *execution* is worth recommending.
    pub fn supports_distributed(self) -> bool {
        !matches!(self, NetworkClass::Consumer)
    }
}

/// One node in the cluster: where it is, and what it has. `assessments` is
/// parallel to `topology.gpus` (computed by the caller, as in the single-box path).
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub name: String,
    /// `cameod` endpoint (e.g. `host:port`) the harness talks to.
    pub address: String,
    pub topology: Topology,
    pub assessments: Vec<TierAssessment>,
}

impl NodeInfo {
    /// Usable VRAM bytes on this node, and whether every GPU reported its VRAM.
    pub(crate) fn usable_vram(&self) -> (u64, bool) {
        let known = !self.topology.gpus.is_empty()
            && self.topology.gpus.iter().all(|g| g.vram_mb.is_some());
        let total: u64 = self
            .topology
            .gpus
            .iter()
            .filter_map(|g| g.vram_mb)
            .map(|mb| mb * 1024 * 1024)
            .sum();
        ((total as f64 * VRAM_HEADROOM) as u64, known)
    }

    pub(crate) fn training_capable(&self) -> bool {
        self.assessments
            .first()
            .map(|a| a.training_supported)
            .unwrap_or(false)
    }
}

/// The cluster the harness manages.
#[derive(Debug, Clone)]
pub struct Cluster {
    pub nodes: Vec<NodeInfo>,
    pub network: NetworkClass,
}

/// Where a workload landed on the fleet.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FleetPlacement {
    /// The whole workload runs on one node (with that node's own placement plan).
    SingleNode {
        node: usize,
        node_name: String,
        plan: PlacementPlan,
    },
    /// The model exceeds any single node; distributed execution across these
    /// nodes is required. **v2 (net-strategy)** — represented, not executed.
    Distributed { nodes: Vec<usize>, note: String },
}

/// A fleet-level placement decision.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FleetPlan {
    pub task: Task,
    pub chosen: FleetPlacement,
    pub notes: Vec<String>,
}

/// Decide where on the fleet to run a workload.
///
/// Policy:
/// 1. Filter to eligible nodes (training needs a Tier 1/2 node).
/// 2. If the model fits a single node, pick the **tightest fit** (smallest node
///    that still fits), leaving larger nodes free for larger models.
/// 3. If nothing fits and the network supports it, return a Distributed decision
///    (v2 data path). On a consumer network, fall back to the largest node with
///    host offload and say so.
pub fn place_on_fleet(
    cluster: &Cluster,
    model: &ModelMeta,
    task: Task,
    settings: &Settings,
) -> Result<FleetPlan, Error> {
    if cluster.nodes.is_empty() {
        return Err(Error::NoNodes);
    }

    let need = match task {
        Task::Inference => model.total_bytes(),
        Task::Training => model.weights_bytes() * TRAINING_FOOTPRINT_MULT,
    };

    let eligible: Vec<usize> = (0..cluster.nodes.len())
        .filter(|&i| match task {
            Task::Training => cluster.nodes[i].training_capable(),
            Task::Inference => true,
        })
        .collect();
    if eligible.is_empty() {
        return Err(Error::NoTrainableNode);
    }

    let mut notes = Vec::new();

    // Tightest single-node fit (smallest usable that still holds the model).
    let mut best_fit: Option<(usize, u64)> = None;
    // Largest node overall, as the fallback.
    let mut largest: Option<(usize, u64)> = None;
    for &i in &eligible {
        let (usable, known) = cluster.nodes[i].usable_vram();
        if largest.is_none_or(|(_, u)| usable > u) {
            largest = Some((i, usable));
        }
        if known && need <= usable && best_fit.is_none_or(|(_, u)| usable < u) {
            best_fit = Some((i, usable));
        }
    }

    if let Some((i, usable)) = best_fit {
        let node = &cluster.nodes[i];
        notes.push(format!(
            "Placed on node '{}': model ~{:.1} GiB fits its ~{:.1} GiB usable VRAM (tightest fit).",
            node.name,
            gib(need),
            gib(usable)
        ));
        let node_plan = plan(&node.topology, &node.assessments, model, task, settings)?;
        return Ok(FleetPlan {
            task,
            chosen: FleetPlacement::SingleNode {
                node: i,
                node_name: node.name.clone(),
                plan: node_plan,
            },
            notes,
        });
    }

    // Nothing fits on a single node.
    if cluster.network.supports_distributed() && eligible.len() > 1 {
        notes.push(format!(
            "Model ~{:.1} GiB exceeds every single node; distributing across {} nodes over a {:?} network (v2 data path via net-strategy).",
            gib(need),
            eligible.len(),
            cluster.network
        ));
        return Ok(FleetPlan {
            task,
            chosen: FleetPlacement::Distributed {
                nodes: eligible,
                note: "distributed execution is v2 (net-strategy); this plan records the intent"
                    .to_string(),
            },
            notes,
        });
    }

    // Consumer network (or a single eligible node): fall back to the largest node
    // with host offload — cross-node sharding would be bandwidth-bound.
    let (i, _) = largest.expect("eligible is non-empty");
    let node = &cluster.nodes[i];
    notes.push(format!(
        "No single node fits the model and the {:?} network makes cross-node sharding bandwidth-bound; \
         falling back to the largest node '{}' with host offload.",
        cluster.network, node.name
    ));
    let node_plan = plan(&node.topology, &node.assessments, model, task, settings)?;
    Ok(FleetPlan {
        task,
        chosen: FleetPlacement::SingleNode {
            node: i,
            node_name: node.name.clone(),
            plan: node_plan,
        },
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::QuantLevel;
    use cameo_gpu_detect::{classify, GpuInfo, OverrideDb};

    fn node(name: &str, gfx: &str, vram_mb: u64, ngpus: usize) -> NodeInfo {
        let db = OverrideDb::embedded();
        let gpus: Vec<GpuInfo> = (0..ngpus)
            .map(|_| GpuInfo {
                model: gfx.into(),
                pci_id: "1002:0000".into(),
                vram_mb: Some(vram_mb),
                gfx_arch: Some(gfx.into()),
                driver_version: None,
                ..Default::default()
            })
            .collect();
        let assessments = gpus.iter().cloned().map(|g| classify(g, &db)).collect();
        NodeInfo {
            name: name.into(),
            address: format!("{name}:9000"),
            topology: Topology::new(gpus, Vec::new()),
            assessments,
        }
    }

    fn cluster(nodes: Vec<NodeInfo>, network: NetworkClass) -> Cluster {
        Cluster { nodes, network }
    }

    #[test]
    fn tightest_fitting_node_wins() {
        // 7B (~4 GiB) fits both; the smaller node should be chosen to leave the big one free.
        let c = cluster(
            vec![
                node("big", "gfx1100", 24576, 1),
                node("small", "gfx1100", 8192, 1),
            ],
            NetworkClass::FastEthernet,
        );
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        let fp = place_on_fleet(&c, &m, Task::Inference, &Settings::default()).unwrap();
        match fp.chosen {
            FleetPlacement::SingleNode { node_name, .. } => assert_eq!(node_name, "small"),
            other => panic!("expected SingleNode, got {other:?}"),
        }
    }

    #[test]
    fn training_skips_tier3_nodes() {
        let c = cluster(
            vec![
                node("old", "gfx803", 8192, 1),   // Polaris = Tier 3
                node("new", "gfx1100", 24576, 1), // Tier 1
            ],
            NetworkClass::FastEthernet,
        );
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        let fp = place_on_fleet(&c, &m, Task::Training, &Settings::default()).unwrap();
        match fp.chosen {
            FleetPlacement::SingleNode { node_name, .. } => assert_eq!(node_name, "new"),
            other => panic!("expected the Tier-1 node, got {other:?}"),
        }
    }

    #[test]
    fn training_with_only_tier3_errors() {
        let c = cluster(
            vec![node("old", "gfx803", 8192, 1)],
            NetworkClass::FastEthernet,
        );
        let m = ModelMeta::dense("x", 7.0, QuantLevel::Q4_K_M);
        assert!(matches!(
            place_on_fleet(&c, &m, Task::Training, &Settings::default()),
            Err(Error::NoTrainableNode)
        ));
    }

    #[test]
    fn oversized_model_distributes_on_fast_network() {
        // A huge model that fits no single 16 GiB node.
        let c = cluster(
            vec![
                node("n0", "gfx1100", 16384, 1),
                node("n1", "gfx1100", 16384, 1),
            ],
            NetworkClass::Infiniband,
        );
        let m = ModelMeta::dense("llama-405b", 405.0, QuantLevel::Q4_K_M);
        let fp = place_on_fleet(&c, &m, Task::Inference, &Settings::default()).unwrap();
        assert!(matches!(fp.chosen, FleetPlacement::Distributed { .. }));
    }

    #[test]
    fn oversized_model_on_consumer_net_falls_back_to_largest() {
        let c = cluster(
            vec![
                node("n0", "gfx1100", 16384, 1),
                node("big", "gfx1100", 24576, 1),
            ],
            NetworkClass::Consumer,
        );
        let m = ModelMeta::dense("llama-405b", 405.0, QuantLevel::Q4_K_M);
        let fp = place_on_fleet(&c, &m, Task::Inference, &Settings::default()).unwrap();
        match fp.chosen {
            FleetPlacement::SingleNode { node_name, .. } => assert_eq!(node_name, "big"),
            other => panic!("expected fallback to largest node, got {other:?}"),
        }
    }
}
