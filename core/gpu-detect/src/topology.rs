//! Multi-GPU topology: how many cards are present and how they're linked.
//!
//! The link type between cards decides which multi-GPU strategies are worth
//! using — a slow host-only path makes cross-card tensor splitting expensive,
//! while XGMI makes it cheap. The parser is pure and fixture-tested; the numbers
//! it feeds into the placement engine are what turn "N cards" into a real plan.

use crate::hostmem::HostMemory;
use crate::types::GpuInfo;

/// How two GPUs are connected, fastest to slowest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// AMD Infinity Fabric (XGMI) — fast, coherent, direct.
    Xgmi,
    /// PCIe peer-to-peer — direct card-to-card DMA over PCIe.
    PcieP2P,
    /// No direct path — traffic goes through host RAM (slow, bandwidth-bound).
    HostOnly,
}

impl LinkKind {
    /// A coarse relative-bandwidth score (higher is better) for ranking strategies.
    pub fn bandwidth_rank(self) -> u8 {
        match self {
            LinkKind::Xgmi => 3,
            LinkKind::PcieP2P => 2,
            LinkKind::HostOnly => 1,
        }
    }
}

/// A link between two GPUs (by their index in [`Topology::gpus`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub a: usize,
    pub b: usize,
    pub kind: LinkKind,
}

/// The set of GPUs and the links between them.
#[derive(Debug, Clone, PartialEq)]
pub struct Topology {
    pub gpus: Vec<GpuInfo>,
    pub links: Vec<Link>,
    /// Host RAM facts when the collector could read them. `None` means unknown,
    /// and the planner must then refuse to assume host offload capacity exists.
    pub host_mem: Option<HostMemory>,
}

impl Topology {
    /// A single-GPU topology (no links).
    pub fn single(gpu: GpuInfo) -> Self {
        Self {
            gpus: vec![gpu],
            links: Vec::new(),
            host_mem: None,
        }
    }

    /// Build from detected GPUs plus parsed links.
    pub fn new(gpus: Vec<GpuInfo>, links: Vec<Link>) -> Self {
        Self {
            gpus,
            links,
            host_mem: None,
        }
    }

    /// Attach host-memory facts to a topology.
    #[must_use]
    pub fn with_host_memory(mut self, host_mem: Option<HostMemory>) -> Self {
        self.host_mem = host_mem;
        self
    }

    pub fn gpu_count(&self) -> usize {
        self.gpus.len()
    }

    pub fn is_multi_gpu(&self) -> bool {
        self.gpus.len() > 1
    }

    /// The link between two cards. Two distinct cards with no recorded link are
    /// assumed [`LinkKind::HostOnly`] (the conservative default).
    pub fn link_between(&self, a: usize, b: usize) -> Option<LinkKind> {
        if a == b {
            return None;
        }
        let found = self
            .links
            .iter()
            .find(|l| (l.a == a && l.b == b) || (l.a == b && l.b == a))
            .map(|l| l.kind);
        Some(found.unwrap_or(LinkKind::HostOnly))
    }

    /// The worst (lowest-bandwidth) link among all card pairs — the bottleneck
    /// that governs whether cross-card strategies are worthwhile. `None` for a
    /// single GPU.
    pub fn bottleneck_link(&self) -> Option<LinkKind> {
        if self.gpus.len() < 2 {
            return None;
        }
        let mut worst = LinkKind::Xgmi;
        for a in 0..self.gpus.len() {
            for b in (a + 1)..self.gpus.len() {
                if let Some(k) = self.link_between(a, b) {
                    if k.bandwidth_rank() < worst.bandwidth_rank() {
                        worst = k;
                    }
                }
            }
        }
        Some(worst)
    }
}

/// Parse `rocm-smi --showtopo` link-type matrix into a list of [`Link`]s.
///
/// Expects a header row of `GPU0 GPU1 ...` and one data row per GPU whose first
/// token is `GPUi` followed by a link token per column. Unknown tokens are
/// skipped; only the upper triangle is emitted (no self, no duplicates).
pub fn parse_rocm_smi_topo(output: &str) -> Vec<Link> {
    let mut header: Vec<usize> = Vec::new();
    let mut links = Vec::new();

    for line in output.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.is_empty() {
            continue;
        }
        // Header: every token is a GPU label.
        if header.is_empty() && toks.len() > 1 && toks.iter().all(|t| gpu_index(t).is_some()) {
            header = toks.iter().filter_map(|t| gpu_index(t)).collect();
            continue;
        }
        // Data row: first token is GPUi, rest are cells aligned to the header.
        let Some(i) = gpu_index(toks[0]) else {
            continue;
        };
        if header.is_empty() {
            continue;
        }
        for (col, cell) in toks[1..].iter().enumerate() {
            let Some(&j) = header.get(col) else { break };
            if j <= i {
                continue; // upper triangle only
            }
            if let Some(kind) = link_kind(cell) {
                links.push(Link { a: i, b: j, kind });
            }
        }
    }
    links
}

fn gpu_index(tok: &str) -> Option<usize> {
    tok.strip_prefix("GPU").and_then(|s| s.parse().ok())
}

fn link_kind(cell: &str) -> Option<LinkKind> {
    match cell.to_uppercase().as_str() {
        "0" => None, // self
        "XGMI" => Some(LinkKind::Xgmi),
        // PIX/PXB/PCIE are P2P-capable PCIe paths (through at most a switch).
        "PCIE" | "PIX" | "PXB" => Some(LinkKind::PcieP2P),
        // PHB/NODE/SYS traverse a host bridge / interconnect: no direct P2P.
        "PHB" | "NODE" | "SYS" => Some(LinkKind::HostOnly),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(idx: u16) -> GpuInfo {
        GpuInfo {
            model: format!("card{idx}"),
            pci_id: format!("1002:00{idx:02x}"),
            vram_mb: Some(16384),
            gfx_arch: Some("gfx1100".into()),
            driver_version: None,
            ..Default::default()
        }
    }

    #[test]
    fn parses_xgmi_matrix() {
        let txt = "\
      GPU0  GPU1
GPU0  0     XGMI
GPU1  XGMI  0";
        let links = parse_rocm_smi_topo(txt);
        assert_eq!(
            links,
            vec![Link {
                a: 0,
                b: 1,
                kind: LinkKind::Xgmi
            }]
        );
    }

    #[test]
    fn host_only_when_phb() {
        let txt = "\
      GPU0  GPU1
GPU0  0     PHB
GPU1  PHB   0";
        let links = parse_rocm_smi_topo(txt);
        assert_eq!(links[0].kind, LinkKind::HostOnly);
    }

    #[test]
    fn bottleneck_is_worst_link() {
        let topo = Topology::new(
            vec![gpu(0), gpu(1), gpu(2)],
            vec![
                Link {
                    a: 0,
                    b: 1,
                    kind: LinkKind::Xgmi,
                },
                Link {
                    a: 0,
                    b: 2,
                    kind: LinkKind::HostOnly,
                },
                Link {
                    a: 1,
                    b: 2,
                    kind: LinkKind::Xgmi,
                },
            ],
        );
        assert_eq!(topo.bottleneck_link(), Some(LinkKind::HostOnly));
    }

    #[test]
    fn missing_link_defaults_host_only() {
        let topo = Topology::new(vec![gpu(0), gpu(1)], vec![]);
        assert_eq!(topo.link_between(0, 1), Some(LinkKind::HostOnly));
        assert_eq!(topo.link_between(0, 0), None);
    }
}
