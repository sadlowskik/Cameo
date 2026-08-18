//! End-to-end fixture test: raw `lspci` + `rocminfo` text -> tier assessment.
//!
//! Fixtures are illustrative captures; the first real Phase 1 run replaces them
//! with ground truth from actual hardware.

use cameo_gpu_detect::{classify, parse, GpuInfo, OverrideDb, Tier};

#[test]
fn rx6700xt_pipeline_yields_tier2() {
    let lspci = include_str!("fixtures/lspci_rx6700xt.txt");
    let rocminfo = include_str!("fixtures/rocminfo_gfx1030.txt");

    let mut gpus = parse::parse_lspci(lspci);
    assert_eq!(
        gpus.len(),
        1,
        "should detect exactly one AMD display device"
    );

    let mut gpu = gpus.remove(0);
    assert_eq!(gpu.pci_id, "1002:73df");
    assert!(
        gpu.model.contains("Radeon RX 6700 XT"),
        "model was {:?}",
        gpu.model
    );

    gpu.gfx_arch = parse::parse_rocminfo_gfx(rocminfo);
    assert_eq!(gpu.gfx_arch.as_deref(), Some("gfx1030"));

    let db = OverrideDb::embedded();
    let assessment = classify(gpu, &db);
    assert_eq!(assessment.tier, Tier::Tier2);
    assert_eq!(assessment.hsa_override.as_deref(), Some("10.3.0"));
    assert!(assessment.training_supported);
    assert!(assessment
        .rationale
        .contains("HSA_OVERRIDE_GFX_VERSION=10.3.0"));
}

#[test]
fn vulkan_only_box_without_rocm_is_tier3() {
    // Same card, but rocminfo produced nothing (no ROCm stack installed).
    let lspci = include_str!("fixtures/lspci_rx6700xt.txt");
    let gpu: GpuInfo = parse::parse_lspci(lspci).remove(0);
    assert!(gpu.gfx_arch.is_none());

    let assessment = classify(gpu, &OverrideDb::embedded());
    assert_eq!(assessment.tier, Tier::Tier3);
    assert!(!assessment.training_supported);
}

/// The regression that single-card fixtures could never catch: on a machine
/// with an integrated *and* a discrete GPU, every card used to be stamped with
/// whichever architecture `rocminfo` happened to print first. Here that is the
/// iGPU's `gfx1103`, which would demote a Tier-1 7900 XTX to Tier 3.
#[test]
fn apu_and_dgpu_each_keep_their_own_architecture_and_tier() {
    let lspci = include_str!("fixtures/lspci_apu_dgpu.txt");
    let rocminfo = include_str!("fixtures/rocminfo_apu_dgpu.txt");

    let mut gpus = parse::parse_lspci(lspci);
    assert_eq!(gpus.len(), 2, "one entry per AMD display device");
    // lspci orders by PCI address, rocminfo by KFD node: the two disagree here,
    // which is exactly what makes positional pairing wrong.
    assert_eq!(gpus[0].pci_id, "1002:744c", "dGPU is first by PCI address");
    assert_eq!(gpus[1].pci_id, "1002:15bf", "iGPU is second");

    let agents = parse::parse_rocminfo_agents(rocminfo);
    assert_eq!(agents[0].gfx, "gfx1103", "iGPU is first by KFD node");
    assert_eq!(parse::correlate_rocm_agents(&mut gpus, &agents), 2);

    assert_eq!(gpus[0].gfx_arch.as_deref(), Some("gfx1100"));
    assert_eq!(gpus[1].gfx_arch.as_deref(), Some("gfx1103"));

    let db = OverrideDb::embedded();
    let dgpu = classify(gpus[0].clone(), &db);
    let igpu = classify(gpus[1].clone(), &db);
    assert_eq!(dgpu.tier, Tier::Tier1, "7900 XTX is officially supported");
    assert!(dgpu.training_supported);
    // gfx1103 is not in the seed database, so it lands on the conservative
    // default rather than borrowing the discrete card's tier.
    assert_eq!(igpu.tier, Tier::Tier3);
}

/// A card whose agent cannot be attributed keeps `gfx_arch = None` and
/// classifies Tier 3. Being conservative costs throughput; being wrong builds a
/// plan for hardware that is not in the machine.
#[test]
fn unattributable_agents_do_not_leak_across_cards() {
    let lspci = include_str!("fixtures/lspci_apu_dgpu.txt");
    let mut gpus = parse::parse_lspci(lspci);
    // Agents with neither Chip ID nor BDFID, and more than one card present.
    let agents = parse::parse_rocminfo_agents(
        "Agent 1\n  Name: gfx1100\n  Device Type: GPU\n\
         Agent 2\n  Name: gfx1103\n  Device Type: GPU\n",
    );
    assert_eq!(agents.len(), 2);
    assert_eq!(parse::correlate_rocm_agents(&mut gpus, &agents), 0);
    for gpu in gpus {
        assert!(gpu.gfx_arch.is_none());
        assert_eq!(classify(gpu, &OverrideDb::embedded()).tier, Tier::Tier3);
    }
}
