# Definition of Done — v1

From plan §8. v1 is done when a user can do all of the following. Multi-node
clustering, Kubernetes, and kernel-level MoE placement are explicitly **not**
required for v1.

- [ ] Boot the Cameo ISO on a machine with any AMD GPU.
- [ ] Get an accurate, plain-language readout of the GPU's support tier at install.
- [ ] `cameo run <model>` serves inference over Vulkan (or ROCm on Tier 1/2) with
      no manual ROCm/Vulkan/driver wrangling.
- [ ] Run a MoE model larger than VRAM would naively allow, via automatic expert
      offloading, without manual configuration.
- [ ] Optionally train (Tier 1/2) over ROCm with the same no-version-pinning experience.
- [ ] Do all of the above via the CLI (`--json` scriptable) and, if Phase 7 is
      reached, a GUI dashboard.
- [ ] Optionally run the same capabilities in a container with correct GPU passthrough.

## Progress marker
The current tree delivers the hardware-independent foundation (detection, tiering,
CLI, API contract, config) and the automated Phase 1 validation runbook. It also
now delivers the **`cameod` control plane** — a browser console (GPU/tier report,
inference-endpoint lifecycle, model cache) that ships in the ISO and starts on
boot — built to the same boundary discipline (pure logic + capture-fixture tests,
Linux-gated I/O), so it is exercised off-hardware. The **Phase 1 gate** — a real
`known-good-combo.json` from `scripts/phase1` on actual AMD hardware — is the next
milestone; Phase 2 backend work is blocked on it.
