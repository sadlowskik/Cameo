# Cameo Architecture (current state)

This documents what the code **actually does today**, not the full vision (that's
`CAMEO_PROJECT_PLAN.md`). Update it as reality changes.

## Shape

Cameo is a Cargo workspace. All logic lives in `core/`; `cli/` is a thin client.
The current tree is the **hardware-independent** slice — detection logic, the API
contract, the CLI, and cross-cutting config — plus stubs for everything that needs
validated hardware.

```
cli/  (cameo)                              core/api  (contract, JSON-RPC types)
   │  detect → classify → plan → act          ▲  shared by cli + future gui
   ▼                                           │
core/gpu-detect ──── core/config ─────────────┘
   │  parse+classify+topology  precedence: flag > file > auto
   ▼
core/placement  (the brain)
   │  (topology × model × task) → PlacementPlan → CommandSpec
   ▼  ── execution boundary (the ONLY hardware-touching call) ──
core/{backend-vulkan, backend-rocm, quant-tools}   thin executors of a CommandSpec
core/{moe-harness, net-strategy}                   stubs (Phase 3 / v2)
```

**The execution boundary.** Everything from detection through
`placement::command::build_*` is pure and unit-tested on any OS: it decides what
to run and produces the exact argv+env. Only `placement::command::execute`
(Linux-gated) spawns a process against the GPU. That concentrates all hardware
risk into one small surface, which the single Phase 1 run validates — and
`cameo … --dry-run` exercises the entire pipeline with no GPU.

## `core/gpu-detect`
Pure, testable detection logic:
- `parse` — `lspci -nn`, `rocminfo`, and sysfs text → `GpuInfo`. No I/O.
- `overrides` — versioned compatibility DB (`data/overrides.toml`, compiled in).
- `classify` — `GpuInfo` + DB → `TierAssessment` (Tier 1/2/3 + rationale + HSA override).
- `collect` — Linux-only I/O that gathers the raw text (`#[cfg(target_os="linux")]`);
  returns `UnsupportedOs` elsewhere.

Key rule: unknown or ROCm-less hardware falls back to **Tier 3 (Vulkan-only)**.
The classifier never invents a ROCm path.

`gpu-detect` also carries **topology** (`topology` module): multiple GPUs plus
the links between them (XGMI / PCIe-P2P / host-only, parsed from
`rocm-smi --showtopo`), and the bottleneck link that governs whether cross-card
strategies are worthwhile.

## `core/config`
`Settings` is a bag of `Option`s; `overlay` merges layers; `resolve(auto, file,
flags)` applies the precedence **auto < file < flag**. Enforces the plan's hard
"auto but overridable" requirement.

## `core/placement` (the brain)
Pure decision logic: `plan(topology, assessments, model, task, settings)` →
`PlacementPlan` (backend, multi-GPU strategy, offload of layers/experts/KV, env).
`model` estimates memory from coarse (calibratable) constants; the *structure* of
the decisions is what's tested. `command::build_*` turns a plan into an exact
`CommandSpec` (argv+env for llama.cpp / torchrun / llama-quantize) — the flag
assumptions live here, centralized for Phase 1 to correct. `command::execute` is
the sole hardware-touching call (Linux-gated).

`fleet::place_on_fleet(cluster, model, task, settings)` recurses the same logic up
to **multiple nodes** — this is how the harness manages a fleet. A `Cluster` is
nodes (each with its own box-topology + tier) plus a `NetworkClass`; the fleet
planner picks the tightest-fitting node (reusing `plan` per node), or records a
`Distributed` decision when no single node fits and the network supports it. Note
the honest split: fleet **orchestration** (which node runs what) lives here;
cross-node **distributed execution** (one model sharded over the network) is the v2
data path in `net-strategy`, represented as a decision but not executed.

`agents::resolve_agent(s)` binds *agents* to compute: an `AgentSpec` names an engine
that is either **Cloud** (a provider endpoint) or **Local** (a model Cameo serves on
a chosen node, `Auto` via the fleet planner or pinned by name). It reuses
`place_on_fleet` + `build_llama_server` to produce, per agent, either the cloud
endpoint or a node + serve command + local endpoint. This is the **binding/placement**
layer for orchestrating many cloud and local agents; *running* the agent loop is the
harness's job (Knossos), and cross-agent coordination stays deliberately thin.

## `core/api`
The stable contract the CLI and GUI both bind to: versioned `Request`/`Response`
envelopes and a `Call` enum (`gpu.status`, `model.run`, `model.quantize`,
`train.start`, `install.plan`). **Types only** today; the Unix-socket transport
lands in Phase 2. See `docs/api.md`.

## `cli/` (`cameo`)
`clap`-based, `--json` and `--dry-run` on every command. `gpu-status` (shows
topology + tiers) / `plan` (compute a plan without running) / `run` / `serve`
(persistent OpenAI-compatible `llama-server` endpoint) / `quantize` / `train` /
`install`. Detection is live on Linux or fed from `--lspci-file` /
`--rocminfo-file` / `--topo-file` on dev machines. `train` refuses on Tier 3
(exit 2); execution errors exit 3. `--dry-run` prints the plan + exact command
without spawning anything. Logs go to stderr, filtered by `CAMEO_LOG`.

## Executors vs. stubs
`backend-vulkan`, `backend-rocm`, `quant-tools` are **thin executors** — they name
the right binary and run a prepared `CommandSpec` through the shared boundary.
They only *do* something on a validated Linux host; elsewhere `execute` returns
`UnsupportedOs`. `moe-harness` (Phase 3) and `net-strategy` (v2) remain stubs.

## `archiso/`
Scaffolded ISO profile: package set + boot-layer tuning (amdgpu module options,
hugepages/sysctl, first-boot tier report). Real files, but built only on Linux.

## Testing
38 unit/integration tests cover parsing, classification, topology, config
precedence, the placement engine (fit/offload/multi-GPU/training-gating), the
command builders, and API serde round-trips — all runnable on any OS. Detection
fixtures in `core/gpu-detect/tests/fixtures/` are **illustrative** until the
first real Phase 1 capture replaces them.
