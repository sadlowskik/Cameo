//! The thin fleet controller (F13, client half).
//!
//! `cameo fleet` turns several `cameod` boxes into one fleet without a scheduler
//! of its own: it polls each node's authenticated `GET /api/node`, rebuilds the
//! [`Cluster`] the placement brain (`fleet.rs`) already consumes, and either shows
//! the fleet or asks the brain where a model should run. Discovery is a static
//! node list for now (mDNS later); the same node description feeds a k8s
//! device-plugin when you outgrow this.
//!
//! Fetching shells out to `curl`, matching the rest of the CLI's
//! external-tool-not-a-linked-HTTP-stack pattern. The JSON→[`NodeInfo`] step is
//! pure and unit-tested; only [`fetch_node`] touches the network.

use anyhow::{anyhow, bail, Result};
use std::process::Command;

use cameo_gpu_detect::{TierAssessment, Topology};
use cameo_placement::{Cluster, NetworkClass, NodeInfo};
use serde::Deserialize;

/// The subset of `GET /api/node` the controller needs to rebuild a node.
#[derive(Deserialize)]
struct NodeDescription {
    name: String,
    topology: Topology,
    gpus: Vec<TierAssessment>,
}

/// Parse an `/api/node` response body into a [`NodeInfo`] at `address`. Pure, so
/// it is unit-tested against a canned body with no network.
fn node_from_json(address: &str, body: &[u8]) -> Result<NodeInfo> {
    let d: NodeDescription = serde_json::from_slice(body)
        .map_err(|e| anyhow!("parsing /api/node from {address}: {e}"))?;
    Ok(NodeInfo {
        name: d.name,
        address: address.to_string(),
        topology: d.topology,
        assessments: d.gpus,
    })
}

/// Fetch one node's self-description over HTTP (via `curl`).
fn fetch_node(address: &str, key: Option<&str>) -> Result<NodeInfo> {
    let url = format!("http://{address}/api/node");
    let mut cmd = Command::new("curl");
    cmd.args(["-s", "--fail", "--max-time", "10"]);
    if let Some(k) = key {
        cmd.arg("-H").arg(format!("Authorization: Bearer {k}"));
    }
    cmd.arg(&url);
    let out = cmd
        .output()
        .map_err(|e| anyhow!("could not run curl (is it installed?): {e}"))?;
    if !out.status.success() {
        bail!(
            "could not reach {url} (curl exit {:?}). Is cameod running there, and is the \
             console key correct?",
            out.status.code()
        );
    }
    node_from_json(address, &out.stdout)
}

/// GET/POST/DELETE a cameod `/api` route. Same curl pattern as [`fetch_node`].
fn api(
    method: &str,
    address: &str,
    path: &str,
    key: Option<&str>,
    body: Option<&str>,
) -> Result<Vec<u8>> {
    let url = format!("http://{address}{path}");
    let mut cmd = Command::new("curl");
    cmd.args(["-s", "--fail", "--max-time", "30", "-X", method]);
    if let Some(k) = key {
        cmd.arg("-H").arg(format!("Authorization: Bearer {k}"));
    }
    if let Some(b) = body {
        cmd.args(["-H", "Content-Type: application/json", "-d", b]);
    }
    cmd.arg(&url);
    let out = cmd
        .output()
        .map_err(|e| anyhow!("could not run curl (is it installed?): {e}"))?;
    if !out.status.success() {
        bail!(
            "{method} {url} failed (curl exit {:?}). Is cameod running, and is the console key set?",
            out.status.code()
        );
    }
    Ok(out.stdout)
}

/// Whether an `/api/engines` body lists `model` among its loaded models. Pure, so
/// the model-match logic is unit-tested without a live node.
fn engines_lists_model(body: &[u8], model: &str) -> Result<bool> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| anyhow!("parsing /api/engines: {e}"))?;
    Ok(v.get("models")
        .and_then(|m| m.as_array())
        .is_some_and(|arr| arr.iter().any(|x| x.as_str() == Some(model))))
}

/// Whether this node already has `model` loaded (one serve, many sessions).
pub fn node_serves(address: &str, key: Option<&str>, model: &str) -> Result<bool> {
    let body = api("GET", address, "/api/engines", key, None)?;
    engines_lists_model(&body, model)
        .map_err(|e| anyhow!("parsing /api/engines from {address}: {e}"))
}

/// Start a serve on `address` unless that model is already warm.
pub fn start_model(address: &str, key: Option<&str>, model: &str) -> Result<String> {
    if node_serves(address, key, model)? {
        return Ok(format!(
            "{address} already serves {model} — reusing the resident /v1 (no second llama-server)"
        ));
    }
    let payload = serde_json::json!({ "model": model }).to_string();
    let body = api("POST", address, "/api/servers", key, Some(&payload))?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Id of the first running endpoint serving `model` in an `/api/servers` body, or
/// `None` if none matches. Pure, so the (occasionally fiddly) nested lookup is
/// unit-tested without a live node.
fn server_id_for_model(body: &[u8], model: &str) -> Result<Option<String>> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| anyhow!("parsing /api/servers: {e}"))?;
    Ok(v.get("servers")
        .and_then(|s| s.as_array())
        .into_iter()
        .flatten()
        .find(|e| e.get("model").and_then(|m| m.as_str()) == Some(model))
        .and_then(|e| e.get("id").and_then(|i| i.as_str()))
        .map(str::to_string))
}

/// Stop the first running endpoint whose model name matches.
pub fn stop_model(address: &str, key: Option<&str>, model: &str) -> Result<String> {
    let body = api("GET", address, "/api/servers", key, None)?;
    let id = server_id_for_model(&body, model)
        .map_err(|e| anyhow!("parsing /api/servers from {address}: {e}"))?
        .ok_or_else(|| anyhow!("no running server for model {model} on {address}"))?;
    api("DELETE", address, &format!("/api/servers/{id}"), key, None)?;
    Ok(format!("stopped {id} ({model}) on {address}"))
}

/// Poll every node address and assemble the [`Cluster`] the placement brain reads.
pub fn build_cluster(
    addresses: &[String],
    key: Option<&str>,
    network: NetworkClass,
) -> Result<Cluster> {
    if addresses.is_empty() {
        bail!("no nodes given; pass at least one --node host:port");
    }
    let mut nodes = Vec::new();
    for addr in addresses {
        nodes.push(fetch_node(addr, key)?);
    }
    Ok(Cluster { nodes, network })
}

/// Map a `--network` value to a [`NetworkClass`]; defaults to the home-lab case.
pub fn network_class(name: &str) -> NetworkClass {
    match name.to_ascii_lowercase().as_str() {
        "infiniband" | "ib" => NetworkClass::Infiniband,
        "fast" | "ethernet" | "datacenter" => NetworkClass::FastEthernet,
        _ => NetworkClass::Consumer,
    }
}

/// A one-line-per-node fleet summary for `cameo fleet status`.
pub fn summarize(cluster: &Cluster) -> String {
    let mut out = String::new();
    for (i, node) in cluster.nodes.iter().enumerate() {
        let gpus = node
            .assessments
            .iter()
            .map(|a| format!("{} (Tier {})", a.gpu.model, a.tier.as_number()))
            .collect::<Vec<_>>()
            .join(", ");
        let gpus = if gpus.is_empty() {
            "CPU-only".to_string()
        } else {
            gpus
        };
        out.push_str(&format!(
            "node {i}: {} @ {}\n  {}\n",
            node.name, node.address, gpus
        ));
    }
    out.push_str(&format!(
        "network: {:?} (distributed execution {})",
        cluster.network,
        if cluster.network.supports_distributed() {
            "viable"
        } else {
            "bandwidth-bound; single-node fits only"
        }
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canned /api/node body, matching what cameod serves.
    const NODE_JSON: &[u8] = br#"{
        "name": "box-a",
        "cameo_version": "0.1.0",
        "topology": {
            "gpus": [{"model":"Radeon RX 7900 XTX","vendor":"amd","pci_id":"1002:744c","vram_mb":24560,"memory":"dedicated","gfx_arch":"gfx1100"}],
            "links": [],
            "host_mem": {"total_bytes": 34359738368, "available_bytes": 20000000000}
        },
        "gpus": [{
            "gpu": {"model":"Radeon RX 7900 XTX","vendor":"amd","pci_id":"1002:744c","vram_mb":24560,"memory":"dedicated","gfx_arch":"gfx1100"},
            "tier":"Tier1","training_supported":true,"rationale":"official"
        }],
        "endpoints": []
    }"#;

    #[test]
    fn engines_body_membership() {
        let body = br#"{"models":["qwen2.5-7b","tinyllama"]}"#;
        assert!(engines_lists_model(body, "qwen2.5-7b").unwrap());
        assert!(!engines_lists_model(body, "not-loaded").unwrap());
        // A body with no models array is simply "serves nothing", not an error.
        assert!(!engines_lists_model(br#"{}"#, "qwen2.5-7b").unwrap());
        assert!(engines_lists_model(b"not json", "x").is_err());
    }

    #[test]
    fn picks_the_server_id_for_a_model() {
        let body = br#"{"servers":[
            {"id":"qwen2.5-7b-8080","model":"qwen2.5-7b"},
            {"id":"tiny-8081","model":"tinyllama"}
        ]}"#;
        assert_eq!(
            server_id_for_model(body, "tinyllama").unwrap().as_deref(),
            Some("tiny-8081")
        );
        // No matching model → None, which the caller turns into a clean error.
        assert_eq!(server_id_for_model(body, "absent").unwrap(), None);
        assert_eq!(server_id_for_model(br#"{"servers":[]}"#, "x").unwrap(), None);
    }

    #[test]
    fn rebuilds_a_node_from_its_description() {
        let node = node_from_json("box-a:9090", NODE_JSON).unwrap();
        assert_eq!(node.name, "box-a");
        assert_eq!(node.address, "box-a:9090");
        assert_eq!(node.topology.gpus.len(), 1);
        assert_eq!(node.assessments.len(), 1);
        assert_eq!(node.assessments[0].gpu.vram_mb, Some(24560));
    }

    #[test]
    fn summarize_lists_each_node_and_the_network() {
        let node = node_from_json("box-a:9090", NODE_JSON).unwrap();
        let cluster = Cluster {
            nodes: vec![node],
            network: NetworkClass::Consumer,
        };
        let s = summarize(&cluster);
        assert!(s.contains("box-a @ box-a:9090"));
        assert!(s.contains("Tier 1"));
        assert!(s.contains("bandwidth-bound"));
    }

    #[test]
    fn network_class_defaults_to_consumer() {
        assert_eq!(network_class("ib"), NetworkClass::Infiniband);
        assert_eq!(network_class("fast"), NetworkClass::FastEthernet);
        assert_eq!(network_class("whatever"), NetworkClass::Consumer);
    }
}
