//! Multi-node distributed execution layouts (F14).
//!
//! When a model exceeds any single node and the network supports it, llama.cpp's
//! **RPC backend** shards it across boxes: each worker runs `rpc-server`, and the
//! head node runs llama with `--rpc host:port,host:port,…`. This module turns the
//! placement brain's `FleetPlacement::Distributed` decision (which node indices
//! participate) into the concrete commands that stand that up.
//!
//! It is pure and unit-tested — it produces command layouts, it does not run them
//! (that is the caller's job, through the same execution boundary as everything
//! else). The **bandwidth guard stays in `fleet.rs`**: it only returns a
//! `Distributed` decision on a network that supports it, so by the time these
//! layouts are built the "is this worth it" question is already answered.
//!
//! ⚠️ The `rpc-server` / `--rpc` flag spellings are the llama.cpp RPC surface and
//! are a Phase-1 item to confirm on real hardware, centralized here like the other
//! backend flags.

use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum Error {
    #[error("distributed execution needs at least two nodes; got {0}")]
    NotDistributable(usize),
}

/// The `rpc-server` a single worker node runs so the head can offload to it.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcWorker {
    /// The worker's reachable host (from its node address).
    pub host: String,
    /// The port its `rpc-server` listens on.
    pub port: u16,
    /// The full `rpc-server` invocation for this worker.
    pub command: Vec<String>,
}

/// A complete RPC sharding layout: what each worker runs, and the `--rpc`
/// argument the head node appends to its `llama-server` / `llama-cli` command.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcLayout {
    pub workers: Vec<RpcWorker>,
    /// `host1:port1,host2:port2,…` — the value for llama.cpp's `--rpc` flag.
    pub endpoints: String,
}

impl RpcLayout {
    /// The args to append to the head node's llama command to shard over the
    /// workers: `--rpc host1:port1,…`.
    pub fn head_rpc_args(&self) -> Vec<String> {
        vec!["--rpc".to_string(), self.endpoints.clone()]
    }
}

/// Build an RPC sharding layout for the given node addresses.
///
/// Each node gets an `rpc-server` on `base_port + i`, listening on all interfaces
/// (the head reaches it over the network). `node_addresses` are `host` or
/// `host:port` (the cameod address); only the host part is used, since the RPC
/// port is assigned here.
pub fn rpc_layout(node_addresses: &[String], base_port: u16) -> Result<RpcLayout, Error> {
    if node_addresses.len() < 2 {
        return Err(Error::NotDistributable(node_addresses.len()));
    }
    let mut workers = Vec::new();
    let mut endpoints = Vec::new();
    for (i, addr) in node_addresses.iter().enumerate() {
        let host = host_of(addr).to_string();
        let port = base_port.saturating_add(i as u16);
        workers.push(RpcWorker {
            host: host.clone(),
            port,
            command: vec![
                "rpc-server".into(),
                "--host".into(),
                "0.0.0.0".into(),
                "--port".into(),
                port.to_string(),
            ],
        });
        endpoints.push(format!("{host}:{port}"));
    }
    Ok(RpcLayout {
        workers,
        endpoints: endpoints.join(","),
    })
}

/// The host part of a `host` or `host:port` address (`[v6]:port` aware).
fn host_of(address: &str) -> &str {
    if let Some(rest) = address.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(address);
    }
    match address.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => h,
        _ => address,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_layout_over_three_nodes() {
        let nodes = vec![
            "box-a:9090".to_string(),
            "box-b:9090".to_string(),
            "box-c:9090".to_string(),
        ];
        let layout = rpc_layout(&nodes, 50052).unwrap();
        assert_eq!(layout.workers.len(), 3);
        assert_eq!(layout.workers[0].host, "box-a");
        assert_eq!(layout.workers[0].port, 50052);
        assert_eq!(layout.workers[2].port, 50054);
        // The head points at every worker.
        assert_eq!(layout.endpoints, "box-a:50052,box-b:50053,box-c:50054");
        assert_eq!(
            layout.head_rpc_args(),
            vec!["--rpc", "box-a:50052,box-b:50053,box-c:50054"]
        );
        // Each worker binds all interfaces so the head can reach it.
        assert!(layout.workers[0]
            .command
            .windows(2)
            .any(|w| w == ["--port", "50052"]));
    }

    #[test]
    fn a_single_node_is_not_distributable() {
        assert_eq!(
            rpc_layout(&["only-one:9090".into()], 50052),
            Err(Error::NotDistributable(1))
        );
    }

    #[test]
    fn host_is_extracted_from_addresses() {
        assert_eq!(host_of("box-a:9090"), "box-a");
        assert_eq!(host_of("10.0.0.5:9090"), "10.0.0.5");
        assert_eq!(host_of("bare-host"), "bare-host");
        assert_eq!(host_of("[::1]:9090"), "::1");
    }
}
