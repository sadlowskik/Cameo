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

**Code-shaped (not a hardware proof):** detection, tiering, CLI, `cameod` console
+ hub, `/v1`, supervisor/VRAM admit, one-serve-per-GGUF in `resolve_agents`,
self-host operator socket, ISO/container delivery, starter GGUF seed.

**Still open vs this list:** boot+serve on a signed `known-good-combo.json`
(hardware), `cameo train` as a productized loop (launcher exists), Secure Boot
signing. MoE userspace offload is code-shaped (`cameo-moe-harness` + placement).

The Phase 1 gate remains: a real combo from `scripts/phase1` on AMD silicon.
