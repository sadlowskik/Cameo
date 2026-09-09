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

pub mod curl;

#[derive(Debug, Error, PartialEq)]
pub enum Error {
    #[error("distributed execution needs at least two nodes; got {0}")]
    NotDistributable(usize),
    #[error("invalid node address '{0}'")]
    InvalidAddress(String),
}

/// Extract the host from a hostname/IP with an optional port. Bracketed IPv6 is
/// supported; a bare IPv6 literal is returned whole. Malformed multi-colon
/// authorities are rejected instead of being reclassified as hostnames.
pub fn host_of(address: &str) -> Option<&str> {
    if address.is_empty() {
        return None;
    }
    if let Some(rest) = address.strip_prefix('[') {
        let close = rest.find(']')?;
        let host = &rest[..close];
        let suffix = &rest[close + 1..];
        if host.parse::<std::net::Ipv6Addr>().is_err()
            || !(suffix.is_empty()
                || suffix
                    .strip_prefix(':')
                    .is_some_and(|port| port.parse::<u16>().is_ok()))
        {
            return None;
        }
        return Some(host);
    }
    if address.parse::<std::net::IpAddr>().is_ok() {
        return Some(address);
    }
    match address.matches(':').count() {
        0 => Some(address),
        1 => {
            let (host, port) = address.rsplit_once(':')?;
            (!host.is_empty() && port.parse::<u16>().is_ok()).then_some(host)
        }
        _ => None,
    }
}

pub fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Parse a literal address while distinguishing hostnames from strings that
/// merely look like malformed IP literals. SSRF guards can reject the latter
/// rather than passing them to DNS as names.
pub fn parse_ip_literal(host: &str) -> Result<Option<std::net::IpAddr>, Error> {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(Some(ip));
    }
    let ipv4_like = host.contains('.')
        && host
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.');
    if host.contains(':') || host.contains('[') || host.contains(']') || ipv4_like {
        Err(Error::InvalidAddress(host.to_string()))
    } else {
        Ok(None)
    }
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
        let host = host_of(addr)
            .ok_or_else(|| Error::InvalidAddress(addr.clone()))?
            .to_string();
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
        assert_eq!(host_of("box-a:9090"), Some("box-a"));
        assert_eq!(host_of("10.0.0.5:9090"), Some("10.0.0.5"));
        assert_eq!(host_of("bare-host"), Some("bare-host"));
        assert_eq!(host_of("[::1]:9090"), Some("::1"));
        assert_eq!(host_of("fe80::1"), Some("fe80::1"));
        assert_eq!(host_of("fe80::1:not-a-port"), None);
    }

    #[test]
    fn malformed_ip_literals_are_not_hostnames() {
        assert_eq!(
            parse_ip_literal("fe80::1"),
            Ok(Some("fe80::1".parse().unwrap()))
        );
        assert_eq!(
            parse_ip_literal("999.999.999.999"),
            Err(Error::InvalidAddress("999.999.999.999".into()))
        );
        assert_eq!(parse_ip_literal("box.local"), Ok(None));
        assert!(is_loopback("::1"));
    }
}
