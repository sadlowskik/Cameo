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
