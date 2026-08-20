//! Agent orchestration: binding agents' engine slots to compute.
//!
//! An "agent" is a harness (e.g. Knossos) instance with *something in its engine
//! slot*. That something is either a **cloud** endpoint or a **local** model that
//! Cameo serves on chosen hardware. This module resolves an [`AgentSpec`] into a
//! concrete [`AgentRunPlan`]: for cloud, the provider endpoint; for local, it
//! reuses [`crate::fleet::place_on_fleet`] to pick a node and
//! [`crate::command::build_llama_server`] to produce the serve command + endpoint.
//!
//! Scope, honestly: this is the **binding + placement** layer. Actually running
//! the agent (spawning the harness, pointing its slot at the endpoint, and any
//! multi-agent coordination) is the harness's job — Cameo places the compute, the
//! harness runs the loop.

use crate::command::{build_llama_server, CommandSpec};
use crate::error::Error;
use crate::fleet::{place_on_fleet, Cluster, FleetPlacement};
use crate::model::ModelMeta;
use crate::plan::{plan, Task};
use cameo_config::Settings;
use serde::Serialize;

/// Where an agent's engine (model) comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineBinding {
    /// A cloud/API model — the harness points its slot at a provider endpoint.
    Cloud { provider: String, model: String },
    /// A local model that Cameo serves on chosen hardware.
    Local {
        /// Path to the GGUF model on the target node.
        path: String,
        model: ModelMeta,
        target: PlacementTarget,
    },
}

/// How to place a local agent's model.
#[derive(Debug, Clone, PartialEq)]
pub enum PlacementTarget {
    /// Let the fleet planner choose the best node.
    Auto,
    /// Pin to a named node.
    Node(String),
}

/// A declarative agent: a name, a role, and where its engine comes from.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSpec {
    pub name: String,
    /// Free-form role/label, e.g. "planner", "coder", "reviewer".
    pub role: String,
    pub engine: EngineBinding,
}

/// A resolved, runnable agent binding.
///
/// ⚠️ For a local agent on a reachable address, `serve` carries the API key as an
/// argument — it has to, that is how `llama-server` receives it. Treat an
/// `AgentRunPlan` as secret-bearing: do not log it or serialize it into anything
/// world-readable.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentRunPlan {
    /// Point the harness at a cloud provider.
    Cloud {
        name: String,
        role: String,
        provider: String,
        model: String,
        endpoint: String,
    },
    /// Serve a local model on a node, then point the harness at it.
    Local {
        name: String,
        role: String,
        node: usize,
        node_name: String,
        endpoint: String,
        /// The address `llama-server` binds to. Loopback unless the node is
        /// genuinely remote, and never the wildcard without authentication.
        bind: String,
        /// Whether the listener requires an API key.
        authenticated: bool,
        /// The `llama-server` command that stands the model up on the node.
        serve: CommandSpec,
        /// The node already serves this GGUF — callers must not spawn `serve`.
        already_running: bool,
    },
}

/// Base URL for a known cloud provider.
fn provider_endpoint(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "anthropic" => Some("https://api.anthropic.com"),
        "openai" => Some("https://api.openai.com/v1"),
        _ => None,
    }
}

/// The host part of a `host:port` (or `[v6]:port`) address.
fn host_of(address: &str) -> &str {
    if let Some(rest) = address.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("127.0.0.1");
    }
    match address.split_once(':') {
        Some((h, _)) => h,
        None => address,
    }
}

/// Whether an address names this machine only.
fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Resolve one agent spec into a run plan. `port` is used only for local serving.
///
/// Serving is fail-closed. `llama-server` has no authentication of its own
/// beyond `--api-key`, so an unauthenticated listener on a routable address is
/// an open completion endpoint — and previously this function bound every local
/// agent to `0.0.0.0` with no key at all. Now a non-loopback node requires
/// `serve_api_key`, and the listener binds to that node's own address rather
/// than every interface on the box.
pub fn resolve_agent(
    spec: &AgentSpec,
    cluster: &Cluster,
    settings: &Settings,
    port: u16,
) -> Result<AgentRunPlan, Error> {
    match &spec.engine {
        EngineBinding::Cloud { provider, model } => {
            let endpoint = provider_endpoint(provider)
                .ok_or_else(|| Error::UnknownProvider(provider.clone()))?;
            Ok(AgentRunPlan::Cloud {
                name: spec.name.clone(),
                role: spec.role.clone(),
                provider: provider.clone(),
                model: model.clone(),
                endpoint: endpoint.to_string(),
            })
        }
        EngineBinding::Local {
            path,
            model,
            target,
        } => {
            let (node_idx, node_plan, node_name) = match target {
                PlacementTarget::Auto => {
                    let fp = place_on_fleet(cluster, model, Task::Inference, settings)?;
                    match fp.chosen {
                        FleetPlacement::SingleNode {
                            node,
                            node_name,
                            plan,
                        } => (node, plan, node_name),
                        FleetPlacement::Distributed { .. } => {
                            return Err(Error::LocalAgentTooLarge(spec.name.clone()))
                        }
                    }
                }
                PlacementTarget::Node(name) => {
                    let node = cluster
                        .nodes
                        .iter()
                        .position(|n| &n.name == name)
                        .ok_or_else(|| Error::NodeNotFound(name.clone()))?;
                    let p = plan(
                        &cluster.nodes[node].topology,
                        &cluster.nodes[node].assessments,
                        model,
                        Task::Inference,
                        settings,
                    )?;
                    (node, p, name.clone())
                }
            };

            let host = host_of(&cluster.nodes[node_idx].address).to_string();
            let loopback = is_loopback(&host);
            let api_key = settings.serve_api_key.as_deref();
            if !loopback && api_key.is_none() {
                return Err(Error::MissingApiKey(spec.name.clone()));
            }

            // Bind to the node's own address when it is a literal IP, so the
            // listener is not exposed on interfaces nobody asked about. A
            // hostname cannot be bound directly, so those fall back to the
            // wildcard — which by now is guaranteed to be authenticated.
            let bind = if host.parse::<std::net::IpAddr>().is_ok() {
                host.clone()
            } else if loopback {
                "127.0.0.1".to_string()
            } else {
                "0.0.0.0".to_string()
            };

            let serve = build_llama_server(
                &node_plan,
                model,
                path,
                "llama-server",
                &bind,
                port,
                api_key,
            );
            let already_running = cluster.nodes[node_idx].is_warm_for(&model.name, path);
            Ok(AgentRunPlan::Local {
                name: spec.name.clone(),
                role: spec.role.clone(),
                node: node_idx,
                node_name,
                endpoint: format!("http://{host}:{port}"),
                bind,
                authenticated: api_key.is_some(),
                serve,
                already_running,
            })
        }
    }
}

/// Resolve a fleet of agents at once.
///
/// A new listen port is allocated only for a **new** (node, model path).
/// Two agents that land on the same node with the same GGUF share one
/// `llama-server` — they share VRAM because they share the process, not
/// because a later residency pass will magically merge them.
pub fn resolve_agents(
    specs: &[AgentSpec],
    cluster: &Cluster,
    settings: &Settings,
) -> Vec<Result<AgentRunPlan, Error>> {
    let mut port: u16 = 8100;
    let mut resident: std::collections::BTreeMap<(String, String), AgentRunPlan> =
        std::collections::BTreeMap::new();
    let mut out = Vec::with_capacity(specs.len());
    for spec in specs {
        let path = match &spec.engine {
            EngineBinding::Local { path, .. } => Some(path.clone()),
            EngineBinding::Cloud { .. } => None,
        };
        let resolved = resolve_agent(spec, cluster, settings, port);
        match (&resolved, path) {
            (
                Ok(AgentRunPlan::Local {
                    node_name,
                    already_running,
                    ..
                }),
                Some(path),
            ) => {
                let key = (node_name.clone(), path);
                if let Some(existing) = resident.get(&key) {
                    out.push(Ok(reuse_local(spec, existing)));
                    continue;
                }
                if let Ok(plan) = &resolved {
                    resident.insert(key, plan.clone());
                }
                // A warm node already paid for the process — do not burn the next port.
                if !already_running {
                    port = port.saturating_add(1);
                }
                out.push(resolved);
            }
            (Ok(AgentRunPlan::Local { .. }), None) => out.push(resolved),
            _ => out.push(resolved),
        }
    }
    out
}

/// Same serve, different agent name/role — the process is already paid for.
fn reuse_local(spec: &AgentSpec, existing: &AgentRunPlan) -> AgentRunPlan {
    match existing {
        AgentRunPlan::Local {
            node,
            node_name,
            endpoint,
            bind,
            authenticated,
            serve,
            ..
        } => AgentRunPlan::Local {
            name: spec.name.clone(),
            role: spec.role.clone(),
            node: *node,
            node_name: node_name.clone(),
            endpoint: endpoint.clone(),
            bind: bind.clone(),
            authenticated: *authenticated,
            serve: serve.clone(),
            already_running: true,
        },
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::{NetworkClass, NodeInfo};
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
            address: format!("{name}.local:9000"),
            topology: Topology::new(vec![gpu], Vec::new()),
            assessments,
            resident: Vec::new(),
        }
    }

    fn cluster() -> Cluster {
        Cluster {
            nodes: vec![
                node("edge", "gfx1030", 16384),
                node("beefy", "gfx1100", 24576),
            ],
            network: NetworkClass::FastEthernet,
        }
    }

    /// Settings for a fleet that is allowed to serve off-box.
    fn keyed() -> Settings {
        Settings {
            serve_api_key: Some("fleet-key".into()),
            ..Default::default()
        }
    }

    fn local_spec(name: &str, target: PlacementTarget) -> AgentSpec {
        AgentSpec {
            name: name.into(),
            role: "coder".into(),
            engine: EngineBinding::Local {
                path: "/models/qwen7b.gguf".into(),
                model: ModelMeta::dense("qwen-7b", 7.0, QuantLevel::Q4_K_M),
                target,
            },
        }
    }

    #[test]
    fn cloud_agent_resolves_to_provider_endpoint() {
        let spec = AgentSpec {
            name: "planner".into(),
            role: "planner".into(),
            engine: EngineBinding::Cloud {
                provider: "anthropic".into(),
                model: "claude-opus-4-8".into(),
            },
        };
        let plan = resolve_agent(&spec, &cluster(), &Settings::default(), 8100).unwrap();
        match plan {
            AgentRunPlan::Cloud { endpoint, .. } => assert!(endpoint.contains("anthropic")),
            other => panic!("expected Cloud, got {other:?}"),
        }
    }

    #[test]
    fn unknown_provider_errors() {
        let spec = AgentSpec {
            name: "x".into(),
            role: "x".into(),
            engine: EngineBinding::Cloud {
                provider: "acme-ai".into(),
                model: "m".into(),
            },
        };
        assert!(matches!(
            resolve_agent(&spec, &cluster(), &Settings::default(), 8100),
            Err(Error::UnknownProvider(_))
        ));
    }

    #[test]
    fn local_agent_auto_places_and_produces_serve() {
        let plan = resolve_agent(
            &local_spec("worker", PlacementTarget::Auto),
            &cluster(),
            &keyed(),
            8100,
        )
        .unwrap();
        match plan {
            AgentRunPlan::Local {
                endpoint, serve, ..
            } => {
                assert!(endpoint.starts_with("http://"));
                assert_eq!(serve.program, "llama-server");
                assert!(serve.args.iter().any(|a| a == "--port"));
            }
            other => panic!("expected Local, got {other:?}"),
        }
    }

    #[test]
    fn local_agent_pins_to_named_node() {
        let plan = resolve_agent(
            &local_spec("w", PlacementTarget::Node("beefy".into())),
            &cluster(),
            &keyed(),
            8100,
        )
        .unwrap();
        match plan {
            AgentRunPlan::Local { node_name, .. } => assert_eq!(node_name, "beefy"),
            other => panic!("expected Local on beefy, got {other:?}"),
        }
    }

    #[test]
    fn missing_node_errors() {
        assert!(matches!(
            resolve_agent(
                &local_spec("w", PlacementTarget::Node("ghost".into())),
                &cluster(),
                &keyed(),
                8100
            ),
            Err(Error::NodeNotFound(_))
        ));
    }

    #[test]
    fn orchestrating_mixed_fleet_assigns_ports_to_locals_only() {
        let specs = vec![
            AgentSpec {
                name: "brain".into(),
                role: "planner".into(),
                engine: EngineBinding::Cloud {
                    provider: "openai".into(),
                    model: "gpt".into(),
                },
            },
            local_spec("hands1", PlacementTarget::Node("edge".into())),
            local_spec("hands2", PlacementTarget::Node("beefy".into())),
        ];
        let plans: Vec<_> = resolve_agents(&specs, &cluster(), &keyed())
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        // Cloud agent, then two local agents on distinct ports 8100 / 8101.
        assert!(matches!(plans[0], AgentRunPlan::Cloud { .. }));
        let ports: Vec<u16> = plans[1..]
            .iter()
            .map(|p| match p {
                AgentRunPlan::Local { endpoint, .. } => {
                    endpoint.rsplit(':').next().unwrap().parse().unwrap()
                }
                _ => panic!("expected local"),
            })
            .collect();
        assert_eq!(ports, vec![8100, 8101]);
    }

    #[test]
    fn two_agents_same_model_same_node_share_one_serve() {
        let specs = vec![
            local_spec("hands1", PlacementTarget::Node("beefy".into())),
            local_spec("hands2", PlacementTarget::Node("beefy".into())),
        ];
        let plans: Vec<_> = resolve_agents(&specs, &cluster(), &keyed())
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let ends: Vec<&str> = plans
            .iter()
            .map(|p| match p {
                AgentRunPlan::Local { endpoint, name, .. } => {
                    assert!(name.starts_with("hands"));
                    endpoint.as_str()
                }
                _ => panic!("expected local"),
            })
            .collect();
        assert_eq!(
            ends[0], ends[1],
            "same GGUF on one node is one llama-server"
        );
        match &plans[1] {
            AgentRunPlan::Local {
                already_running, ..
            } => assert!(already_running),
            _ => panic!("expected local"),
        }
    }

    #[test]
    fn a_node_already_serving_the_model_does_not_spawn() {
        let mut c = cluster();
        c.nodes[1].resident = vec!["qwen-7b".into()];
        let plan = resolve_agent(
            &local_spec("w", PlacementTarget::Node("beefy".into())),
            &c,
            &keyed(),
            8100,
        )
        .unwrap();
        match plan {
            AgentRunPlan::Local {
                already_running, ..
            } => assert!(already_running),
            other => panic!("expected Local, got {other:?}"),
        }
    }

    // ---- serving is fail-closed --------------------------------------------

    #[test]
    fn remote_node_without_an_api_key_is_refused() {
        let err = resolve_agent(
            &local_spec("w", PlacementTarget::Node("beefy".into())),
            &cluster(),
            &Settings::default(),
            8100,
        )
        .unwrap_err();
        assert!(matches!(err, Error::MissingApiKey(_)), "got {err:?}");
    }

    #[test]
    fn a_reachable_listener_is_always_authenticated() {
        let plan = resolve_agent(
            &local_spec("w", PlacementTarget::Node("beefy".into())),
            &cluster(),
            &keyed(),
            8100,
        )
        .unwrap();
        match plan {
            AgentRunPlan::Local {
                bind,
                authenticated,
                serve,
                ..
            } => {
                assert!(authenticated);
                assert!(serve
                    .args
                    .windows(2)
                    .any(|w| w == ["--api-key", "fleet-key"]));
                // The wildcard is only ever reached with a key in hand.
                if bind == "0.0.0.0" {
                    assert!(authenticated);
                }
            }
            other => panic!("expected Local, got {other:?}"),
        }
    }

    #[test]
    fn a_loopback_node_needs_no_key_and_never_binds_the_wildcard() {
        let mut c = cluster();
        c.nodes[0].address = "127.0.0.1:9000".into();
        let plan = resolve_agent(
            &local_spec("w", PlacementTarget::Node("edge".into())),
            &c,
            &Settings::default(),
            8100,
        )
        .unwrap();
        match plan {
            AgentRunPlan::Local {
                bind,
                authenticated,
                ..
            } => {
                assert_eq!(bind, "127.0.0.1");
                assert!(!authenticated);
            }
            other => panic!("expected Local, got {other:?}"),
        }
    }

    #[test]
    fn host_parsing_handles_ipv6_and_bare_hosts() {
        assert_eq!(host_of("[::1]:9000"), "::1");
        assert_eq!(host_of("beefy.local:9000"), "beefy.local");
        assert_eq!(host_of("beefy.local"), "beefy.local");
        assert!(is_loopback("::1"));
        assert!(is_loopback("localhost"));
        assert!(!is_loopback("10.0.0.4"));
    }
}
