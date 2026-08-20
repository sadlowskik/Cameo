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
    route, Candidate, ModelMeta, NodeInfo, NodeLoad, QuantLevel, RouteChoice, RouteError,
    RouteRequest, Task,
};
use serde::Deserialize;
use serde_json::Value;

/// The dispatch request a harness POSTs to `/hub/dispatch`.
#[derive(Deserialize)]
pub struct DispatchBody {
    pub model: String,
    #[serde(default = "default_params")]
    pub params: f64,
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
}

fn default_params() -> f64 {
    7.0
}
fn default_quant() -> String {
    "Q4_K_M".into()
}
fn default_port() -> u16 {
    8080
}

impl DispatchBody {
    fn model_meta(&self) -> ModelMeta {
        let quant = QuantLevel::parse(&self.quant).unwrap_or(QuantLevel::Q4_K_M);
        if self.moe {
            ModelMeta::moe(&self.model, self.params, quant)
        } else {
            ModelMeta::dense(&self.model, self.params, quant)
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
}

/// A node reconstructed from its stored description, kept alongside the `node_id`
/// the hub pushes work back to.
struct ParsedNode {
    node_id: String,
    info: NodeInfo,
    load: NodeLoad,
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
    Some(ParsedNode {
        node_id: node_id.to_string(),
        info: NodeInfo {
            name,
            address: String::new(), // not needed for routing; push uses node_id
            topology,
            assessments,
        },
        load: load_from_endpoints(desc.get("endpoints")),
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
pub struct Dispatch {
    pub choice: RouteChoice,
    pub node_id: String,
}

/// Decide where a task should run, given the raw online roster
/// (`(node_id, /api/node description)` pairs) and the request.
pub fn decide(roster: &[(String, Value)], body: &DispatchBody) -> Result<Dispatch, RouteError> {
    let parsed: Vec<ParsedNode> = roster
        .iter()
        .filter_map(|(id, desc)| parse_node(id, desc))
        .collect();
    let candidates: Vec<Candidate> = parsed
        .iter()
        .map(|p| Candidate {
            node: &p.info,
            load: p.load.clone(),
        })
        .collect();
    let choice = route(&candidates, &body.route_request())?;
    Ok(Dispatch {
        node_id: parsed[choice.index].node_id.clone(),
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
            params: 7.0,
            quant: "Q4_K_M".into(),
            moe: false,
            task: None,
            min_tier: None,
            execute,
            port: 8080,
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
}
