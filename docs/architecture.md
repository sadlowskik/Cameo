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

`router::route(candidates, req)` (`core/placement/src/router.rs:139`) is the **live-usage**
axis `place_on_fleet` lacks. Where the fleet planner reasons from *static* topology (does
this node's card qualify, does the model fit its VRAM), `route` also reads what each node
is *currently running* and ranks accordingly. A `Candidate` (`router.rs:40`) pairs a
`&NodeInfo` with a `NodeLoad` (`router.rs:25`: the model names it serves + committed VRAM
bytes). Ranking is a total order (`Ranked::better_than`, `router.rs:122`):

1. **warm** — a node already serving the requested model wins outright (no reload, no extra
   VRAM), then
2. **most free VRAM** — free = `usable_vram() − used_vram_bytes` (`router.rs:90`), then
3. **fewest served endpoints** as a tiebreak (`Ranked.endpoints` is `load.serving.len()`).

Eligibility gates before ranking (`card_ok`, `router.rs:96`): training requires a
training-capable node (`NodeInfo::training_capable`, `fleet.rs:65`); `min_tier: Some(t)`
requires the node's top card to be tier `t` **or better** (lower tier *number* is a better
card); `min_tier: None` admits any card, even a CPU-only node. A candidate is kept only if
it is warm, its VRAM is unknown (treated as roomy — the router *advises* and the node's own
admission check is the real gate, `router.rs:158`), or the model's `need` fits its free VRAM.
`RouteError` (`router.rs:71`) distinguishes `NoCandidates` (empty roster) from
`NoneEligible` (nodes exist, none satisfy card + fit). Pure and unit-tested off-hardware.

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
- `app` — routing (`app::route`, `app.rs:143`) and the reuse glue. Its top-level
  dispatch is layered by credential class, not just by path — see the route table
  and the auth model below.
- `auth` — role-based credentials and deployment posture (its own section below).
- `supervisor` — the one owner of live child processes: a `Mutex<HashMap>` of
  tracked endpoints, each reaped on read (`try_wait`) so a server that died on its
  own reports `exited`, not a false `running`. A spawn that fails is still recorded
  (state `failed`, reason attached) so the dashboard shows *why* nothing came up —
  which on a non-Linux dev host is every start, making the console fully
  demonstrable off-hardware. Also tracks VRAM residency (`vram_need`/`vram_budget`)
  and exposes `served_models`/`endpoint_for_model`/`touch` for the `/v1` gateway.
- `proxy` — the outbound HTTP client for the `/v1` gateway: `forward` (buffered)
  and `forward_streaming` (relays upstream SSE frames as they arrive), presenting
  the backend serve key to the supervised `llama-server`.
- `sessions` — the Knossos "deck": a `Board` (`Mutex<BTreeMap>`) of live harness
  sessions, surfaced under `/api/sessions`.
- `hub` + `agent` + `dispatch` — the fleet hub, node-side phone-home agent, and
  task-dispatch router (their own sections below).
- `dashboard` — two embedded HTML/CSS/JS pages, vanilla, no build step:
  `INDEX_HTML` (`dashboard.rs:9`) the per-node console served at `/`, and
  `HUB_HTML` (`dashboard.rs:485`) the fleet dashboard served at `/` in hub mode.

Detection is live on Linux or replayed from the same `--lspci-file` / … captures
the CLI accepts, so the whole console (GPU report, planning, endpoint list) works
on a dev box with only the final spawn reporting "Linux only".

### Route table (`app::route`, `app.rs:143`)

Dispatch is ordered by auth class. `check_serve_auth` (`app.rs:242`) gates `/v1`
with any **consumer-or-better** key; `check_auth` (`app.rs:342`) gates `/api` and
the admin `/hub` routes with an **operator** key; `check_farm_auth` (`app.rs:399`)
gates node enrollment with the **farm token**. "Open when unconfigured" means the
gate is skipped only when no key of that class exists (loopback dev).

| Route | Method | Auth | Purpose |
|---|---|---|---|
| `/` | GET | none (shell) | console (`INDEX_HTML`) or fleet dashboard (`HUB_HTML`) in hub mode (`app.rs:153`) |
| `/healthz` `/readyz` | GET | none | liveness / readiness (readiness = can detect+plan; served from a 5s detect cache) |
| `/version` | GET | none | daemon version for the console's update check |
| `/metrics` | GET | operator* | Prometheus scrape: supervisor + GPU gauges (`app.rs:171`) |
| `/v1/models`, `/v1/*` | GET/POST | consumer+ | OpenAI-compatible gateway, routed by body `model` to a supervised endpoint (`app.rs:256`) |
| `/api/gpus` `/api/node` `/api/engines` `/api/models` | GET | operator | detection report, self-description, harness engine descriptor, model catalog |
| `/api/plan` | POST | operator | placement preview |
| `/api/servers` (+`{id}`) | GET/POST/DELETE | operator | endpoint lifecycle |
| `/api/sessions` (+`{id}`) | GET/POST/DELETE | operator | Knossos session board |
| `/api/models/gc`, `/api/models/{name}` | POST/DELETE | operator | model-cache management |
| `/hub/register` `/hub/heartbeat` | POST | farm token | node enrollment / liveness (hub mode only) |
| `/hub/nodes` (+`{id}`) | GET/DELETE | operator | fleet roster / forget a node |
| `/hub/nodes/{id}/servers` (+`{sid}`) | POST/DELETE | operator | push serve/stop down to a node's own `/api` |
| `/hub/dispatch` | POST | operator | route a task across the fleet, advise or serve |

\*`/metrics` is gated by the operator key when one is configured, so an
all-interfaces bind does not leak GPU inventory to LAN peers; open on a keyless
dev daemon. `/hub/*` returns 404 when this daemon is not a hub (`app.rs:201`).

## Roles + posture auth (`cameod/src/auth.rs`)

The daemon separates **who may use a model** from **who may manipulate the GPU**.

- `Role` (`auth.rs:27`): `Operator` ⊇ `Consumer`. A consumer credential reaches
  inference (`/v1`) only; an operator credential reaches the control surface
  (`/api`, `/hub`, dispatch, start/stop). `is_operator` requires an operator match;
  `is_consumer_or_better` accepts either (an operator may also run inference).
- `KeyRing` (`auth.rs:75`): the set of accepted keys, each role-tagged. Matching is
  constant-time (`ct_eq`, `auth.rs:82` — only key *length* is observable). `requires_operator`
  / `requires_consumer` (`auth.rs:101`) report whether any key of that class exists;
  when none does, that surface is **open** (loopback dev). The app-layer `check_*`
  functions consult these — the "open when unconfigured" rule lives there, not here.
- `Posture` (`auth.rs:43`): `SelfHost` (default) vs `MultiTenant`. `allows_local_harness`
  (`auth.rs:64`) is `true` only for `SelfHost` — the posture that would grant a
  co-located harness keyless operator power over the privileged local socket.
  **Note:** as of this pass that privileged local socket is *not present in this tree* —
  `Posture` is consulted (surfaced in `/api/engines`, enforced at startup) but the
  keyless-local-operator grant it describes is not yet wired to a route. See Drift.
- `load_keys_file` (`auth.rs:134`): parses `[{key, role, label}]` JSON; roles
  `operator|admin` and `consumer|user|inference`; rejects empty keys and unknown roles.

**Keyring assembly and startup gates** (`main.rs`): the `--console-key` becomes the
primary operator key (`main.rs:173`), `settings.serve_api_key` becomes a consumer key
(`main.rs:180`), and `--keys-file` entries are appended (`main.rs:187`). Three fail-closed
checks then run before binding:

- non-loopback bind with no operator key → refuse (`main.rs:198`);
- `multi-tenant` posture with no operator key → refuse (`main.rs:208`);
- `--hub` with no `--farm-token` → refuse (`main.rs:217`).

`/api/engines` (`app.rs:773`) publishes the *non-secret* posture view a harness needs:
`openai_base_path`, `auth_required` (= `requires_consumer`), served models, and
`local_harness` (= `allows_local_harness`). Serve keys are never serialized here.

## Fleet hub — HiveOS-style phone-home (`cameod/src/hub.rs`, `agent.rs`)

This **inverts** the CLI's `cameo fleet` model. `cameo fleet` *pulls*: a controller
polls a static list of node addresses (`GET /api/node`). The hub *is pushed to*: each
node dials out and registers itself, so the hub never needs inbound access to a node to
learn it exists. The self-description on the wire is the same `/api/node` body either way;
only the initiator flips (`hub.rs:1`).

**Hub side — the `Farm` registry (`hub.rs:68`).** In-memory `Mutex<BTreeMap<String, Enrolled>>`,
same shape as the session `Board`. Staleness is computed on read, not on a timer:

- `ONLINE_WINDOW = 45s` (`hub.rs:27`): a node phoned home within this counts online.
- `DROP_AFTER = 15min` (`hub.rs:33`): a node silent past this is pruned entirely; between
  the two windows it lingers greyed-out on the dashboard.
- **No node cap** (`hub.rs:13`) — a deliberate decision tied to the sustainable-OSS funding
  model; the self-hosted hub is free and unlimited.

Methods: `register` (`hub.rs:87`) derives a stable `node_id` via `pick_id`
(explicit id → name → address → `"node"`, `hub.rs:182`) and upserts, so a node keeps one row
across restarts; `heartbeat` (`hub.rs:108`) refreshes liveness and (if the beat carries one)
the live description, returning **`false` for an unknown node** so its agent re-enrolls;
`remove` (`hub.rs:126`) admin-forgets; `push_target` (`hub.rs:145`) returns a node's
callback `(address, key)` for a push; `online_descriptions` (`hub.rs:133`) yields the
`(node_id, /api/node)` roster the dispatch router consumes (offline nodes excluded);
`list` (`hub.rs:153`) is the dashboard view (online flag, age, GPU summary lifted from the
stored description). Both read paths `prune` first.

**Node side — the agent (`agent.rs`).** When `--hub-url` is set (`main.rs:238`), `agent::spawn`
(`agent.rs:150`) runs `run` (`agent.rs:106`) on a background thread. It `POST`s a
`registration_body` to `/hub/register`, then heartbeats every `HEARTBEAT_SECS = 15`
(`agent.rs:37`, comfortably under the 45s window). It **re-registers** on either a
`known:false` heartbeat response or any transport failure (`agent.rs:132`). All network I/O
shells out to `curl` (`agent.rs:68`) — matching the CLI's external-tool stance; body builders
are pure and unit-tested. The `describe` closure is `app::node_report` (`app.rs:707`), called
fresh on every beat so the hub sees live endpoints; on a non-Linux dev host it yields `None`
and the node still enrolls, just without a hardware description.

**Hub routes (`route_hub`, `app.rs:420`).** `register`/`heartbeat` are farm-token gated
(`check_farm_auth`, `app.rs:399` — fails closed with 403 if no farm token is configured).
The roster/admin routes (`nodes` list+delete, `nodes/{id}/servers` push, `dispatch`) are
operator gated. A push (`push_to_node` → `node_call`, `app.rs:500`) calls the target node's
own authenticated `/api` over `curl`, using the callback address and key it registered with —
the hub is just an HTTP client here, exactly like `cameo fleet`.

## Key path: harness delegation (`POST /hub/dispatch`)

The end-to-end "delegate this task to a box" trace — the seam a harness (Knossos) drives.

1. A harness `POST`s a `DispatchBody` (`dispatch.rs:20`: `model`, `params`, `quant`, `moe`,
   `task`, `min_tier`, `execute`, `port`) to `/hub/dispatch`. Operator gated (`app.rs:487`).
2. `api_dispatch` (`app.rs:551`) pulls the online roster via `farm.online_descriptions()`
   and calls `dispatch::decide` (`dispatch.rs:140`).
3. `decide` reconstructs each node with `parse_node` (`dispatch.rs:89`): it deserializes
   `topology` + `gpus` (tier assessments) back into a `NodeInfo`, and reads live load from
   the `endpoints` array (`load_from_endpoints`, `dispatch.rs:112`: only `state == "running"`
   endpoints count; their `model` names → `serving`, their `vram_bytes` → `used_vram_bytes`).
   A description lacking topology/assessments (a dev node that enrolled without detection) is
   **skipped, not fatal** (`dispatch.rs:89`).
4. It calls `route` (the placement router above) and returns the winning `node_id` +
   `RouteChoice`.
5. Back in `api_dispatch`: `execute:false` → advise only (`executed:false`, node, reason).
   `execute:true` + warm → reuse the resident endpoint, return its `/v1` URL. `execute:true`
   + cold → `node_call` pushes `POST /api/servers` to the chosen node and returns its `/v1`
   endpoint. `NoneEligible` maps to **409** (fleet can't take the work), a malformed body to
   400 (`app.rs:562`).

Note the address in the returned `endpoint` URL is the node's registered *callback* address
(`app.rs:581`); `parse_node` deliberately leaves `NodeInfo.address` empty because routing
keys pushes by `node_id`, not by the routing-time address (`dispatch.rs:102`).

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

## Drift (code vs. prior docs/comments)

- **The `cameod` section above was stale before this pass.** Prior to it, the doc
  listed only `http`/`app`/`supervisor`/`dashboard` and an `/api/{gpus,models,plan,servers}`
  route set. The tree already contained the `/v1` SSE gateway (`proxy`, commit `1107fad`),
  the Knossos session board (`sessions`, commit `d42a903`), and now the fleet hub, auth
  roles, and dispatch (commit `2c2ba31`). "Not documented" there meant "not yet written up,"
  not "not present." This pass reconciles the module list and route table with the code.
- **Privileged local socket — described, not present.** `auth.rs:11` and the `--posture`
  help (`main.rs:64`) describe a self-host box granting a co-located harness *keyless*
  operator power "via the privileged local socket." No such Unix/local socket exists in
  this tree — the daemon binds one TCP listener (`main.rs:260`) and every operator route
  goes through `check_auth`, which only ever consults the keyring. `Posture` today has two
  observable effects: the startup gate that forces an operator key in `multi-tenant`
  (`main.rs:208`), and the `local_harness`/`posture` fields reported by `/api/engines`. An
  auditor should treat the "keyless local harness" grant as **design intent not yet
  implemented**, and not assume a local caller is silently trusted.
- **`/api/node` doc comment names `cameo fleet`, but the live consumer is the hub agent.**
  `api_node`'s comment (`app.rs:679`) says a `cameo fleet` controller polls it; that pull
  path exists, but in this tree the actual in-repo caller of the self-description is
  `app::node_report` feeding the phone-home agent (`main.rs:256`). Not a contradiction —
  both consume the same body — but the pull controller is external/CLI-side, not what drives
  this daemon at runtime.
- **`route`'s "fewest endpoints" tiebreak counts served models.** `Ranked.endpoints`
  (`router.rs:118`) is populated from `load.serving.len()`, i.e. the number of running
  endpoints that reported a model name, not the raw endpoint count. `load_from_endpoints`
  (`dispatch.rs:112`) only pushes to `serving` when an endpoint has a `model` field, so a
  running endpoint with no model name contributes to `used_vram_bytes` but not to the
  tiebreak. Minor and benign; noted so the comment isn't read as raw-endpoint counting.
- **CPU-only / VRAM-unknown nodes are advisory, not admitted.** `route` treats a node whose
  VRAM is unknown (including a CPU-only node, `usable_vram` → `(0,false)`) as "roomy" and
  eligible for inference (`router.rs:158`). This is intentional (the node's own admission is
  the real gate), but it means a dispatch decision can name a node that then rejects the
  serve. The 409-vs-serve-failure split in `api_dispatch` (`app.rs:562`, `app.rs:616`)
  handles that, but auditors should not read a successful *route* as a guaranteed *fit*.

## Not covered

This pass focused on the commit-`2c2ba31` surface: `hub.rs`, `agent.rs`, `dispatch.rs`,
`auth.rs`, `router.rs`, and the routing/startup changes in `app.rs` / `main.rs`, plus the
two-line `fleet.rs`/`lib.rs` exports that support them. Read for interface and control flow,
not line-by-line: `supervisor.rs`, `proxy.rs`, `sessions.rs`, `http.rs`, and the embedded
`dashboard.rs` HTML/JS (only the two exported constants and the `/` routing were verified).
The `core/placement` planner internals (`plan.rs`, `model.rs`, `command.rs`, `agents.rs`)
were not re-audited here beyond confirming the types `router.rs`/`dispatch.rs` consume.

## Testing
Unit and integration tests cover parsing, agent-to-card correlation, classification,
topology, host-memory sizing, config
precedence, the placement engine (fit/offload/multi-GPU/training-gating), the
command builders, API serde round-trips, model-spec resolution, the daemon's HTTP
request parser, and the supervisor's lifecycle (spawn-failure recorded, stop,
relaunch on a freed port) — all runnable on any OS. Detection
fixtures in `core/gpu-detect/tests/fixtures/` are **illustrative** until the
first real Phase 1 capture replaces them.
