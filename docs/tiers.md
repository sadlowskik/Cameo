# GPU Support Tiers

Cameo never silently fails on unsupported hardware. Every install classifies the
detected AMD GPU into one of three tiers and says so in plain language.

| Tier | What it means | Inference | Training |
|------|---------------|-----------|----------|
| **1** | ROCm officially supports this card. | Yes (ROCm, Vulkan fallback) | Yes |
| **2** | No official ROCm support, but it works with a `HSA_OVERRIDE_GFX_VERSION`. | Yes | Community-tested, not guaranteed |
| **3** | No usable ROCm path at all. | Yes (Vulkan only) | No |

**Vulkan is the universal baseline.** Every tier can run inference over Vulkan.
ROCm only ever makes things *faster* on cards that support it — nothing in Cameo
requires ROCm to function.

## How a tier is decided
1. `gfx` architecture is read from `rocminfo` (e.g. `gfx1030`).
2. It's looked up in the compatibility database
   (`core/gpu-detect/data/overrides.toml`).
   - Found → that entry's tier (and, for Tier 2, its known-good HSA override).
   - Not found → **Tier 3** (conservative default).
3. No `gfx` at all (no ROCm stack) → **Tier 3**.

## Overriding the decision
The tier is a smart default, not a verdict. If you know a working ROCm path for a
card Cameo classified as Tier 3, set an explicit `hsa_override` (and backend) in
config — CLI flags and config files both win over auto-detection
(`auto < file < flag`).

## Checking your tier
```bash
cameo gpu-status          # human-readable
cameo gpu-status --json   # machine-readable
```

> The seed database is **illustrative** until real hardware confirms each entry
> via `scripts/phase1`. Treat Tier 2 override values as "try this," not gospel,
> until they carry a `known-good-combo.json` behind them.
