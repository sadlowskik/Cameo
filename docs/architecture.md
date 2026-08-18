# Cameo Architecture (current state)

This documents what the code **actually does today**, not the full vision (that's
`CAMEO_PROJECT_PLAN.md`). Update it as reality changes.

## Shape

Cameo is a Cargo workspace. All logic lives in `core/`; `cli/` is a thin client.
The current tree is the **hardware-independent** slice — detection logic, the API
contract, the CLI, and cross-cutting config — plus stubs for everything that needs
validated hardware.

```
cli/ (cameo) ┐         cameod/ (control plane: daemon + browser dashboard)
             │  both front ends: detect → classify → plan → act
             ▼                                    core/api (contract, JSON-RPC types)
core/gpu-detect ── core/config ── core/models     ▲  shared by cli + future gui
   │  detection boundary          name → .gguf     │
   │  detect_topology (live | replayed captures) ──┘
   ▼
core/placement  (the brain)
   │  (topology × model × task) → PlacementPlan → CommandSpec
   ▼  ── execution boundary: spawn (tracked) | execute (blocking) ──
core/{backend-vulkan, backend-rocm, quant-tools}   thin executors of a CommandSpec
core/{moe-harness, net-strategy}                   stubs (Phase 3 / v2)
```

Two front ends bind to the same brain. The **CLI** runs one plan to completion;
the **daemon** (`cameod`) keeps the processes it starts, which is the only
capability the CLI lacks — hence the execution boundary exposing both a blocking
`execute` and a non-blocking `spawn`.

**Two hardware boundaries.** Cameo has one *execution* boundary and one
*detection* boundary, and conflating them hid real bugs for a while.

`placement::command::{spawn, execute}` (Linux-gated) are the only calls that
spawn a workload against the GPU. `execute` blocks (the CLI's foreground run);
`spawn` hands back the live `Child` without waiting (the daemon's supervisor
tracks it). Both funnel through one `configured_command` that ties the child to
the parent's lifetime with `PR_SET_PDEATHSIG`, so a killed `cameo`/`cameod`
never leaks a `llama-server` still holding VRAM. Everything from `plan` through
`placement::command::build_*` is pure and unit-tested on any OS: it decides what
to run and produces the exact argv+env, so `cameo … --dry-run` exercises the
entire pipeline with no GPU.

`gpu_detect::collect` is the other one. It shells out to `lspci`, `rocminfo` and
`rocm-smi`, and reads `/sys/class/drm` and `/proc/meminfo`. It runs no workload,
but every fact the planner reasons from enters here — which is exactly why an
unacknowledged boundary was expensive: two defects (an architecture attributed
to the wrong card, VRAM read from the wrong device) lived in code that the docs
said did not touch hardware. Its parsing half is pure and fixture-tested; only
the gathering is Linux-gated.

## `core/gpu-detect`
Pure, testable detection logic:
- `parse` — `lspci -nn`, `rocminfo`, and sysfs text → `GpuInfo`. No I/O. Agents
  are matched to cards by **key** (`Chip ID`, then `BDFID`, then a single
  unambiguous leftover), never by position: the three sources order cards
  independently, so an APU + dGPU box gets each card its own architecture, and a
  card that cannot be attributed stays `None` rather than borrowing another's.
- `hostmem` — `/proc/meminfo` → `HostMemory`. The planner cannot size host
  offload without knowing whether host RAM exists.
- `memfacts` — captured `/sys/class/drm` memory facts (TOML), so the memory
  planner can be exercised off-hardware. Dev/testing only.
- `overrides` — versioned compatibility DB (`data/overrides.toml`, compiled in).
- `classify` — `GpuInfo` + DB → `TierAssessment` (Tier 1/2/3 + rationale + HSA override).
- `collect` — Linux-only I/O that gathers the raw text (`#[cfg(target_os="linux")]`);
  returns `UnsupportedOs` elsewhere. Resolves each card's DRM node by PCI
  address, and records whether its memory is dedicated or shared with the host.
- `detect` — the assembly order (`detect_topology(&Captures)`): live via `collect`
  when no capture is given, or replayed step-for-step from captured text on any
  OS. This is where the correlation rules live *once*; both the CLI and the daemon
  drive detection through it rather than re-implementing the glue.

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

## `core/models`
Model acquisition and cache resolution, shared by both front ends so there is one
cache layout and one alias table. `resolve(name)` turns an alias or bare name into
a local `.gguf` path (paths pass through untouched); `pull(spec, report)` fetches
one by shelling out to `curl` (an alias, a `https://` URL, or `owner/repo:file.gguf`),
writing to a `.part` sidecar and renaming on success. `aliases()` / `cached_models()`
return data, never printing — presentation belongs to the caller (the CLI prints a
table; the daemon serves JSON).

## `core/placement` (the brain)
Pure decision logic: `plan(topology, assessments, model, task, settings)` →
`PlacementPlan` (backend, multi-GPU strategy, offload of layers/experts/KV, env).
`model` estimates memory from coarse (calibratable) constants; the *structure* of
the decisions is what's tested. `command::build_*` turns a plan into an exact
`CommandSpec` (argv+env for llama.cpp / torchrun / llama-quantize) — the flag
assumptions live here, centralized for Phase 1 to correct. `command::execute` is
the *execution* boundary (Linux-gated) — the only call that spawns a workload,
though not the only one that touches hardware. `plan` refuses a model that
exceeds VRAM + host RAM instead of emitting a command the kernel would
OOM-kill; `--allow-oversize` overrides it.

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
`--rocminfo-file` / `--topo-file` / `--meminfo-file` / `--gpu-mem-file` on dev
machines. `train` needs `--script` (Cameo launches training, it does not supply
the loop); `serve` refuses a non-loopback `--host` without `--api-key`. `train`
refuses on Tier 3
(exit 2); execution errors exit 3; an oversized model exits 4 and an invalid
model description exits 5. `--dry-run` prints the plan + exact command
without spawning anything. Logs go to stderr, filtered by `CAMEO_LOG`.

## `cameod/` (`cameod`) — the control plane
The browser-administered console: one binary that serves a self-contained
dashboard and a small JSON API over the same detection/placement brain, and
supervises the model endpoints it starts. No external web framework — matching
the project's dependency-light stance (the CLI shells out to `curl` rather than
linking an HTTP stack; the daemon ships its own minimal server).
- `http` — a deliberately tiny HTTP/1.1 server: `GET`/`POST`/`DELETE`,
  `Content-Length` bodies, `Connection: close`, one thread per connection, a body
  cap and I/O timeouts. It only moves bytes; who may reach the port is the app's
  decision.
- `app` — routing and the reuse glue. `GET /api/gpus` (detect + classify),
  `GET /api/models`, `POST /api/plan` (preview), and `GET|POST|GET{id}|DELETE{id}
  /api/servers` (the endpoint lifecycle). Mirrors the CLI's safety rules: a
  non-loopback endpoint without an api key is refused; `/api/*` is gated by a
  console key when one is configured.
- `supervisor` — the one owner of live child processes: a `Mutex<HashMap>` of
  tracked endpoints, each reaped on read (`try_wait`) so a server that died on its
  own reports `exited`, not a false `running`. A spawn that fails is still recorded
  (state `failed`, reason attached) so the dashboard shows *why* nothing came up —
  which on a non-Linux dev host is every start, making the console fully
  demonstrable off-hardware.
- `dashboard` — the single embedded HTML/CSS/JS page; vanilla, no build step.

Detection is live on Linux or replayed from the same `--lspci-file` / … captures
the CLI accepts, so the whole console (GPU report, planning, endpoint list) works
on a dev box with only the final spawn reporting "Linux only".

## Executors vs. stubs
`backend-vulkan`, `backend-rocm`, `quant-tools` are **thin executors** — they name
the right binary and run a prepared `CommandSpec` through the shared boundary.
They only *do* something on a validated Linux host; elsewhere `execute` returns
`UnsupportedOs`. `moe-harness` (Phase 3) and `net-strategy` (v2) remain stubs.

## `archiso/`
Scaffolded ISO profile: package set + boot-layer tuning (amdgpu module options,
first-boot tier report). Real files, but built only on Linux. `scripts/build-iso.sh`
stages the profile, merges releng's airootfs, strips its boot-time remote-execution
channel, and compiles **both** front ends (the `cameo` CLI and the `cameod` daemon)
as the invoking user rather than as root, staging both binaries into the image.
Three Cameo units are enabled: `cameo-firstboot.service` (the tier report, which
now also prints the console URL + key), `cameo-console-init.service` (generates a
random key and an all-interfaces bind into `/run/cameo/cameod.env` each boot,
ordered before the daemon), and `cameod.service` (the control plane). The layered
`EnvironmentFile`s mean the box is a key-protected **home console** out of the box
— open it from your own machine's browser — while `/etc/cameo/cameod.env` lets an
operator override (force loopback, pin a fixed key). So a booted Cameo box *is* the
console — not a dev-box binary you run by hand.

## Testing
Unit and integration tests cover parsing, agent-to-card correlation, classification,
topology, host-memory sizing, config
precedence, the placement engine (fit/offload/multi-GPU/training-gating), the
command builders, API serde round-trips, model-spec resolution, the daemon's HTTP
request parser, and the supervisor's lifecycle (spawn-failure recorded, stop,
relaunch on a freed port) — all runnable on any OS. Detection
fixtures in `core/gpu-detect/tests/fixtures/` are **illustrative** until the
first real Phase 1 capture replaces them.
