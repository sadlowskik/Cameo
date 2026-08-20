# Cameo product plan — the road to v1

Status date: 2026-08-18. First real-hardware boot passed (v0.4.3 on an AMD APU:
boot → console → correct GPU/tier/VRAM detection → `cameo pull`). This document
is the plan to turn what exists into a **finished v1 product** — "most of what we
need" to run AI on almost any machine, manage it like a product, and scale a box
into a fleet.

**Progress (synced against the tree 2026-08-18):** 🟢 done — F1, F2, F4, F6, F7,
F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18 (**16/18**: serving core,
multi-vendor detection, full fleet path, harness surface, usable console,
reproducible-pin, CI smoke, docs). 🟡 implemented, hardware/CI-gated — F3 (Secure
Boot: documented + fallback works; signing pipeline needs an SB machine), F5
(update: `/api/version` + contract done; `cameo-update` wrapper + registry publish
pending). Everything code-shaped is built, green (fmt/clippy/tests), and — where
possible — demonstrated off-hardware; what remains needs Linux/CI/real hardware to
verify, not more code here. Foundation (fleet brain, agents,
supervisor, control-plane HTTP + dashboard, CI, containers passthrough,
`cameo-install`, phase1 toolkit) is in. Detection is corroborated on silicon; the
execution/serving flags are still unvalidated on a real GPU. Now working the
remainder in the sequenced order below.

**Audit checkpoint (2026-08-18):** ran architect + security + runtime auditors
over the fleet/hub work. Access-control core held (no consumer→operator path,
constant-time compares, startup gates fail closed). Five hub-boundary findings
remediated in `fbdf57c` (all behind the shared farm token): registry memory caps
+ LRU eviction + heartbeat pruning, link-local SSRF guard on hub→node push,
first-credential-wins against node-row hijack, `/v1` fail-closed on a routable
bind, and a push timeout under the inbound IO budget. One perf note deferred
(per-dispatch roster re-parse — linear only at hundreds of nodes, fine for a
self-hosted fleet). Workspace green: fmt + clippy `-D warnings` + ~187 tests.

Method (same as the gap assessment): every feature carries **why/damage**,
**fixable?**, **compatibility** (what it must not break), and a
**best-of-both-worlds** approach that resolves the tension instead of picking a
side. Direction is fixed: multi-vendor, container-first, k8s-adopt-when-scaling,
portable daemon as the hero artifact. Aligns with `CAMEO_PROJECT_PLAN.md`.

---

## What "finished v1" means (definition of done)

A machine — laptop, workstation, or server, any major GPU or none — can:

1. **Install Cameo three ways from one build:** container image (hero), bootable
   ISO appliance, or bare install via `cameo-install`.
2. **Run on what it has:** AMD (validated), NVIDIA/Intel via the Vulkan universal
   path, or CPU-only — auto-detected, never a hard failure.
3. **Serve any pulled model** behind one stable **OpenAI-compatible endpoint**,
   with models that **persist** and a `pull` that never fills RAM by surprise.
4. **Be operated like a product:** a console to browse/pull/quantize/remove
   models, start/stop endpoints, watch VRAM and tokens/sec; endpoints that
   **auto-restart** and **share a GPU** under a residency manager.
5. **Join a fleet as a node** — self-describing, authenticated, routed behind one
   front door — at home-lab scale today, k8s-ready for later.
6. **Be shippable:** Secure-Boot-friendly, reproducibly built, updatable, and
   CI-verified to actually boot and serve.
7. **Be built on:** a documented API a harness (Knossos) points its engine slot
   at — the hook the AI-native-platform ambition hangs off.

---

## Already solid (don't rebuild)

- Correctness + hardening cluster (findings #1–9): implemented, tested, and the
  detection half now corroborated on real silicon.
- **Fleet placement brain** (`fleet.rs`) and **agent binding** (`agents.rs`):
  fully implemented + tested, fail-closed auth for off-box serving.
- **Endpoint supervisor** (`cameod/supervisor.rs`): owns live `llama-server`
  children, `PR_SET_PDEATHSIG` so a dead daemon never leaks VRAM.
- **Control-plane HTTP + dashboard** (`http.rs`, `dashboard.rs`): `/api/gpus`,
  `/api/models`, `/api/plan`, `/api/servers` lifecycle, bearer-gated.
- CI (`ci.yml`): `fmt` + `clippy -D warnings` + `test` + `shellcheck` every push.
- Thoughtful package set; installer, wifi, editor/man all present.
- `core/containers` GPU-passthrough recipe (tested); `scripts/phase1/` HW toolkit.

---

## Feature plan

Format per feature: **why/damage · fixable · compatibility · best-of-both**.
Status tags: 🔴 missing · 🟡 partial/scaffolded · 🟢 exists, needs surfacing.

### Area 1 — Delivery & portability

**F1 · Container image of Cameo** 🟢 *(done — `containers/Containerfile`, vulkan+rocm variants, CI smoke)*
Why: `containers/` is notes; `core/containers` passes GPUs to *guest* containers,
not package Cameo. No `Containerfile` exists, so "container-first" has no artifact
and can't run on constrained hardware or join a cluster. · Fixable: yes, mostly
assembly over the existing cargo+package build. · Compat: CUDA and ROCm userspace
can't co-install; Arch base gives ISO parity but rolls. · **Best-of-both:**
Vulkan universal base image + thin `-rocm`/`-cuda` variants; build FROM Arch with
a **pinned package snapshot** (parity + reproducibility); a shared build script
feeds both ISO and Containerfile so they never drift.

**F2 · Model persistence** 🟢 *(done — `/var/lib/cameo/models` default + pull space preflight)*
Why: `cameo pull` defaults to `$HOME/.cache/cameo/models` = RAM overlay on a live
boot; `config` separately defaults to `/var/lib/cameo/models` — the two disagree,
and a live USB fills RAM. · Fixable: yes, easily. · Compat: live vs installed vs
container each want a different default; `CAMEO_MODELS_DIR` stays authoritative. ·
**Best-of-both:** environment-aware default (live → prompt once for a data
partition, wired into `cameo-install`; installed/container → `/var/lib/cameo`),
plus a `pull` **preflight** that compares free space to the model size (already
computed in `model.rs`) and refuses with the planner's friendly oversize message.
Reconcile the two conflicting defaults.

**F3 · Secure Boot support** 🟡 *(documented — `docs/secure-boot.md` shim+signed-chain design, MOK enrollment, and the working disable-SB fallback; the signing pipeline needs an SB machine to build+validate, so it is planned not verified)*
Why: archiso's `systemd-boot`/`syslinux` are unsigned; with Secure Boot on, the
firmware silently rejects the USB and falls through. Every SB-on machine is a
non-starter without disabling firmware settings. · Fixable: yes, established
pattern. · Compat: signing keys/`shim` add build complexity; must not break the
BIOS path. · **Best-of-both:** ship the **`shim` + signed `systemd-boot`** chain
(shim is Microsoft-signed, chains to your key) so SB-on boots unmodified;
document the "disable SB" fallback for anyone who prefers it. Containers sidestep
this entirely (host owns boot) — another reason container-first.

**F4 · Reproducible builds** 🟢 *(commit+SOURCE_DATE_EPOCH identity + opt-in Arch archive snapshot pin `CAMEO_ARCH_SNAPSHOT` / `--build-arg SNAPSHOT`, ISO+container in lockstep; base-image digest pin + a CI build to confirm remain)*
Why: ISO label/version derive from build date, so the same source ≠ the same
image; hard to verify or roll back. · Fixable: yes. · Compat: Arch is rolling —
needs a pinned package snapshot to be truly reproducible. · **Best-of-both:**
drive identity from the **commit + `SOURCE_DATE_EPOCH`** (already the intended
path in `profiledef.sh`) and pin an Arch archive snapshot; publish a checksum
manifest. Same snapshot pin serves F1's container.

**F5 · Update mechanism** 🟡 *(daemon `GET /api/version` + `docs/updating.md` per-delivery contract done; the `cameo-update` wrapper + registry publish are delivery-layer follow-ons)*
Why: no story for updating an installed Cameo or a running container; a product
that can't update safely isn't finished. · Fixable: yes, per delivery. · Compat:
must not break a serving box mid-flight; installed vs container differ. ·
**Best-of-both:** container → **immutable image tags** (`cameo:1.x`, pull-and-
restart, trivial rollback); installed → pinned `pacman` transactions with a known-
good snapshot; ISO → re-flash. `cameod` exposes a version/health endpoint so the
console can flag "update available".

### Area 2 — Hardware breadth

**F6 · Multi-vendor detection + backends** 🟢 *(detection done — NVIDIA `10de` + Intel `8086` recognized and routed to the Vulkan path; per-vendor CUDA accel stays container-first)*
Why: whole stack is AMD-only (one non-AMD mention in the tree); NVIDIA/Intel get
"no AMD GPU detected". · Fixable: yes, layered; `gpu-detect` is cleanly
structured and unit-testable with fixtures, **no hardware needed**. · Compat:
Vulkan is the universal layer (already ships); per-vendor accel can't co-install;
bare-metal NVIDIA (proprietary driver + SB signing) is far harder than in a
container. · **Best-of-both:** generalize `gpu-detect` (NVML/`nvidia-smi` + Intel
branches) as pure logic now; **land vendor breadth in the container variants
first** (host owns the driver) and keep the bare-metal ISO AMD-validated. Add
`backend-cuda`; Vulkan already covers NVIDIA/Intel baseline.

**F7 · CPU backend as first-class fallback** 🟢 *(done — `Backend::Cpu`, `detect_topology_or_cpu`, planner routes to system RAM)*
Why: CPU-only inference landed (git log), but it must be an explicit, planner-
aware backend so a GPU-less or oversized case degrades cleanly instead of failing.
· Fixable: yes; mostly surfacing + selection logic. · Compat: must interact with
the planner's fit math (#3) — CPU means system-RAM budget, not VRAM. ·
**Best-of-both:** treat CPU as the always-available floor; the planner already
knows host RAM (`hostmem.rs`) — route to CPU with a clear "low throughput" note
(the note text already exists in `plan.rs`) rather than refusing.

### Area 3 — Serving runtime (turns one box into a serving product)

**F8 · Unified OpenAI gateway** 🟢 *(done — `/v1/models` + `/v1/*` routed by model name to the serving llama-server, dependency-free proxy)*
Why: per-endpoint OpenAI serving exists (`llama-server`), but there's **no single
front door** — a client must know each model's host/port. A product needs one
stable endpoint. · Fixable: yes; `cameod` already has the HTTP server + the
supervisor's endpoint registry. · Compat: must preserve the fail-closed auth from
`agents.rs`; one door, many models. · **Best-of-both:** extend `cameod` to expose
`/v1/*` and **route by model name** to the right supervised `llama-server` (and,
in a fleet, the right node via `place_on_fleet`). Same gateway serves one box and
the cluster — no separate router to build later.

**F9 · Endpoint robustness: auto-restart + health/readiness** 🟢 *(done — supervised restart w/ backoff + cap, `/healthz` + `/readyz`)*
Why: the supervisor reaps a dead `llama-server` but doesn't restart it; no
health/readiness endpoints (k8s needs them). · Fixable: yes, small. · Compat:
must not restart-loop a genuinely broken command. · **Best-of-both:** supervised
restart with backoff + a crash cap (then park as `failed` with the reason, which
the dashboard already renders); add `/healthz` (process up) and `/readyz` (model
loaded) — the same probes k8s and the F13 controller consume.

**F10 · VRAM residency manager** 🟢 *(done — admission + LRU eviction over the supervisor, planner VRAM math, refuses oversize)*
Why: `agents.rs:224` promises "cameod's residency manager arbitrates that at
runtime," but `supervisor.rs` has no VRAM accounting or eviction — two models on
one GPU will just OOM. · Fixable: yes, moderate. · Compat: must use the same
VRAM/fit math as the planner so decisions agree. · **Best-of-both:** an
admission + **LRU eviction** layer over the supervisor: on start, check the
detected VRAM budget (reuse `gpu-detect` + planner math), evict the
least-recently-used idle endpoint if needed, else refuse with the oversize
message. One source of truth for "what fits," shared with the planner.

**F11 · Observability: metrics + structured logs** 🟢 *(done — `/metrics` Prometheus text: daemon/endpoint/GPU gauges, zero deps)*
Why: no tokens/sec, VRAM, latency, or uptime surfaced beyond process state; can't
operate a serving box blind. · Fixable: yes. · Compat: keep the dependency-light
ethos (no heavy telemetry stack). · **Best-of-both:** a `/metrics` endpoint in
Prometheus text format (zero deps, universally scrapable) fed by the supervisor +
`llama-server` stats; the console reads the same endpoint for its live tiles.

**F12 · Model management: remove / GC / disk usage / quantize** 🟢 *(CLI done — `cameo model ls/du/rm/gc`; console actions + quantize-to-fit pending under F18)*
Why: `pull`/`list`/aliases exist and `quant-tools` exists, but no remove, no disk
accounting, no visible quantize workflow — the cache grows unbounded. · Fixable:
yes, small. · Compat: must respect `CAMEO_MODELS_DIR` and F2's persistence. ·
**Best-of-both:** `cameo model rm/gc/du` + console actions; surface `quant-tools`
as a "quantize to fit this box" action driven by the planner's fit math.

### Area 4 — Cluster (scale a box into a fleet)

**F13 · Node self-description + thin composing controller** 🟢 *(done — authenticated `GET /api/node` + `cameo fleet status|place` that polls nodes, rebuilds the `Cluster`, and runs `place_on_fleet`)*
Why: `fleet.rs` (placement) and `agents.rs` (binding) are real and tested, but
`Cluster`/`NodeInfo` are hand-built structs — **no discovery, no network
transport, no front door**. The brain has no senses. · Fixable: yes; small
first step. · Compat: must carry `agents.rs`'s fail-closed auth over the network;
work at home-lab scale without k8s, yet not preclude it. · **Best-of-both — a
two-tier design where the home-lab path and the k8s path share the same node
truth:** (1) add one authenticated `GET /api/node` route to the existing
`http.rs` returning this box's `gpu-detect` topology + tier + endpoints — makes a
box joinable by *anything*. (2) A thin `cameo fleet` controller takes a static
node list (discovery via mDNS later), polls `/api/node` to **build the `Cluster`
struct `fleet.rs` already consumes**, and fronts it with the F8 gateway. (3) When
you scale, **k8s replaces only the scheduling role**: the same `/api/node`
feeds a device-plugin, `fleet.rs` becomes a scheduler-extender giving model-fit
hints — no rewrite, because both tiers consume the same node description + brain.

**F14 · Distributed execution for oversized models** 🟢 *(layout done — `net-strategy::rpc_layout` emits the `rpc-server` workers + head `--rpc host:port,…`; `cameo fleet place` prints it on a Distributed decision. Live sharded run is hardware-gated)*
Why: `FleetPlacement::Distributed` records intent but nothing shards a model
across nodes; a model bigger than your largest box can't run. · Fixable: yes,
largest/latest. · Compat: bandwidth-bound on consumer networks (`fleet.rs`
already refuses to shard there — keep that). · **Best-of-both:** wire
`net-strategy` to emit **llama.cpp RPC** layouts (`rpc-server` + `--rpc host:port,
…`) when `Distributed` fires on a fast network; ties into `moe-harness`
(currently a Phase-3 stub). Independent of k8s.

### Area 5 — Platform hook (the north-star bridge)

**F15 · Harness (Knossos) integration surface** 🟢 *(done — `GET /api/engines` safe discovery surface + `docs/harness-integration.md` worked example; secret-bearing resolver stays server-side)*
Why: `agents.rs` is literally built to point a harness's "engine slot" at Cameo-
served compute, but there's no exposed, documented API + example to actually do
it — the bridge to the AI-native-platform ambition is latent. · Fixable: yes,
mostly exposure + docs. · Compat: same auth model; stable API version
(`core/api` already versions). · **Best-of-both:** expose `resolve_agent`/
`resolve_agents` over the F8 gateway as a stable endpoint + ship a worked Knossos
example ("here's your fleet, here's the engine URLs, go"). Keeps Cameo the
substrate; the harness owns the loop.

### Area 6 — Quality & docs

**F16 · CI boot-smoke + container smoke** 🟢 *(container smoke in ci.yml; QEMU/OVMF ISO boot-smoke added to iso.yml — informational until the first run calibrates the serial markers, then flip to required)*
Why: `iso.yml` builds but never boots the ISO; boot/serve regressions ship
untested. · Fixable: yes, cheap. · Compat: CI runners have no GPU — software only.
· **Best-of-both:** QEMU/OVMF **boot-smoke** on every ISO build (boot → console →
tier report → CPU-backend token); container build+run smoke; real-GPU accel stays
the gated `scripts/phase1/` checklist.

**F17 · Docs** 🟢 *(done — `quickstart.md` (container/ISO/install), `api-reference.md` (the cameod HTTP surface), harness + updating guides, linked from README; architecture boundary drift already fixed)*
Why: `architecture.md` has the known detection-boundary drift (#18); no single
user-facing quickstart/install/API guide. · Fixable: yes. · **Best-of-both:** fix
the architecture doc to name both hardware boundaries (execution *and* detection),
add a task-oriented quickstart + API reference generated from `core/api` types so
docs can't drift from the contract.

**F18 · Console/UX to a real management surface** 🟢 *(done — chat playground over `/v1`, model delete/GC, GPU/tier + endpoint lifecycle; live metric tiles + fleet view are optional polish)*
Why: `dashboard.rs` lists GPUs/models/servers; v1 wants the `CAMEO_PROJECT_PLAN`
GUI scope — chat playground, model management, VRAM/throughput tiles, fleet view.
· Fixable: yes, incremental over the existing dashboard + F8/F11 endpoints. ·
**Best-of-both:** grow the current dependency-light dashboard (no SPA framework)
against the new `/v1`, `/metrics`, `/api/node` endpoints — one console for one
box and the fleet.

---

## Sequenced roadmap (dependency-ordered; each phase ends demoable)

**Phase A — Runs anywhere, serves a token.** F1 container (Vulkan base + rocm
variant) · F2 persistence · F16 container/QEMU smoke.
*Exit:* `run cameo:vulkan` serves a token on any machine; ISO boot-smoked in CI.

**Phase B — A real serving daemon (single box).** F8 gateway · F9 restart+health ·
F10 residency · F11 metrics · F12 model mgmt.
*Exit:* one OpenAI URL, many models, auto-restart, VRAM-arbitrated, observable.

**Phase C — Runs on anything.** F6 multi-vendor detect+backends · F7 CPU floor.
*Exit:* AMD/NVIDIA/Intel/CPU auto-selected; ISO stays AMD-validated.

**Phase D — Shippable.** F3 Secure Boot · F4 reproducible · F5 update · F17 docs.
*Exit:* SB-on boots; byte-reproducible builds; safe updates; docs match code.

**Phase E — Usable by humans.** F18 console (playground, model mgmt, tiles).
*Exit:* the product is driveable without the CLI.

**Phase F — Fleet.** F13 node self-description + thin controller · F14 RPC.
*Exit:* `cameo fleet` fronts several boxes behind one door; big models shard on a
fast network; k8s-ready.

**Phase G — Platform.** F15 harness surface + Knossos example.
*Exit:* a harness points its engine slot at a Cameo fleet from a documented API.

Continuous: F16 smoke on every build; real-GPU `scripts/phase1/` checklist per HW.

---

## Compatibility matrix (delivery × capability)

| Capability | Container (hero) | ISO appliance | Bare install |
|---|---|---|---|
| Multi-vendor GPU | easy (host driver) | AMD-validated; others later | host-dependent |
| Secure Boot | N/A (host boots) | needs shim (F3) | host-managed |
| Persistence | host volume | data partition (F2) | disk (F2) |
| Update | image tag (F5) | re-flash | pacman snapshot (F5) |
| Cluster node | native (F13) | native (F13) | native (F13) |

## Open decisions (call before building)
1. **Container base:** Arch + pinned snapshot (parity + reproducibility —
   recommended) vs conventional/distroless.
2. **Bare-metal vendor scope:** ISO stays AMD-only, breadth via containers first
   (recommended) vs push NVIDIA/Intel into the ISO too.
3. **Live-medium persistence:** prompt once at first boot, wired into
   `cameo-install` (recommended) vs silent auto-pick.
4. **Gateway auth model:** shared bearer token (simple, recommended for v1) vs
   mTLS between controller and nodes (stronger, heavier) — for F8/F13.
