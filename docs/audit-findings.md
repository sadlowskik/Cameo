# Cameo audit — findings and remediation plan

Status date: 2026-08-18. Scope: full tree (`core/`, `cli/`, `cameod/`, `archiso/`,
`containers/`, `scripts/`, CI). Baseline: `cargo build --workspace` and
`cargo test --workspace` are green (148 tests pass). This document lists concrete
defects and improvements found by reading the code against its own stated design,
then sequences the fixes so they don't introduce new problems.

**Implementation status (2026-08-18):** Batches 1–4 are landed and green
(fmt · clippy `-D warnings` · 154 tests · shellcheck `-x` across every script,
airootfs included). Fixed: A1 (shared `/var/lib/cameo/models`, unit override
removed, installer creates it wheel-writable), A2 (cross-model port-conflict
refusal), A3 (stable-uptime restart-budget reset), B1 (constant-time key
compare), B2 (32 KiB head cap + 64-connection ceiling), B3 (`/metrics` behind
the console key when configured), C1 (unused `cameo-api`/`cameo-moe-harness`
deps dropped; README wording), C2 (`model_dir` now honored via env export in
both front ends), D1 (repo URL unified to `sadlowskik/Cameo`), D2 (airootfs
scripts shellchecked in CI; all pre-existing findings fixed), E4 (5 s detection
cache for `/readyz` + `/metrics`). Also landed: release binaries stripped
(storage), plain-HTTP pulls refused with guidance, `model rm` rejects
path-shaped names, dashboard escaping made attribute-safe + CPU backend option,
installer timezone fallback actually fires. Still open: E1 (streaming gateway),
E2 (Phase-1 calibration — hardware-gated), E3 (live-medium data-partition
prompt), C3 (`core/containers` consumer).

## What Cameo is (audit summary)

An Arch Linux respin plus a Rust workspace that runs, **serves**, and (on capable
AMD cards) trains LLMs, meeting hardware "where it is": Vulkan is the universal
baseline, ROCm the optional accelerator, CPU the always-available floor.

- **`core/`** — pure logic, hardware-independent and heavily unit-tested. GPU
  detection + tiering (`gpu-detect`), the placement brain (`placement`: memory
  budget math, offload/multi-GPU/fleet decisions, command building), model cache
  (`models`), config precedence (`config`).
- **`cli/` (`cameo`)** — thin client over core; `gpu-status/plan/run/serve/pull/
  quantize/train/model/fleet/install`, all `--json`, all `--dry-run`.
- **`cameod/`** — dependency-free HTTP control plane: browser console, `/api/*`,
  the OpenAI `/v1` gateway, an endpoint supervisor (auto-restart, VRAM residency/
  LRU eviction), `/metrics`, `/healthz`/`/readyz`, `/api/node` for fleet.
- **Delivery** — bootable ISO (`archiso/` + `scripts/build-iso.sh`), container
  (`containers/Containerfile`, Vulkan base + rocm variant), bare install
  (`cameo-install`). The execution and detection hardware boundaries are the only
  Linux-only, un-CI-testable surfaces; everything above is exercised off-hardware.

The architecture is clean: one detection/planning brain, two thin front ends, a
real execution boundary with `PR_SET_PDEATHSIG` orphan-proofing, fail-closed auth
for off-box serving. The findings below are localized, not structural.

---

## Findings

### A. Correctness bugs

**A1 — `cameod.service` pins models to the RAM overlay, and installed systems
inherit it.**
`archiso/airootfs/etc/systemd/system/cameod.service:21` sets
`Environment=CAMEO_MODELS_DIR=/root/.cache/cameo/models`. Because
`cameo_models::models_dir()` treats `CAMEO_MODELS_DIR` as the highest-precedence
source, this overrides the intended persistent `/var/lib/cameo/models`. Three
consequences:
- On the **live ISO**, `/root` is a RAM overlay — models pulled from the console
  land in RAM, the exact failure F2 was meant to end. (The `pull` space preflight
  still guards a too-large pull, so this is degraded, not catastrophic.)
- `cameo-install` copies this unit **verbatim** to the installed disk
  (`cameo-install:257-259`), so an **installed** box also writes models to
  `/root/.cache` instead of the persistent `/var/lib/cameo/models`.
- The console's model list (daemon, this env) and a user's `cameo pull` (their own
  `$HOME`, no such env) resolve to **different** directories — they disagree, which
  is precisely the drift F2 claimed to resolve.

*Fix:* drop the `Environment=CAMEO_MODELS_DIR=…` line so `models_dir()`'s
precedence picks `/var/lib/cameo/models`, and ensure that directory exists (a
`tmpfiles.d` entry or `ExecStartPre=/usr/bin/install -d`). If a live-only cache
path is wanted, gate it to the live medium rather than baking it into the unit
that installs to disk.

**A2 — Port collisions across different models are not detected.**
`Supervisor::start` rejects `PortInUse` only when an endpoint with the same id
(`<model-slug>-<port>`) is already running (`supervisor.rs:292-297`). Two
*different* models started on the *same* port both spawn `llama-server`; the
second fails to bind, then consumes its 5 auto-restarts and parks `failed` — with
a generic crash message, not "port already in use". *Fix:* before spawn, reject if
any *running* endpoint already holds `req.port` (regardless of model), with a clear
error.

**A3 — Restart cap is lifetime, not windowed.**
`Endpoint::restarts` (`supervisor.rs:144`) only ever increments; after
`MAX_RESTARTS` (5) total crashes across the endpoint's whole life it parks `failed`
permanently. A stable server that crashes once every few days is eventually killed
for good. *Fix:* reset the counter after a sustained uptime (e.g. running > N
seconds since last restart), or track restarts in a sliding window.

### B. Security / hardening

**B1 — Non-constant-time bearer-key comparison.**
Both `check_auth` and `check_serve_auth` (`app.rs:234-245`, `167-178`) compare with
`presented == Some(key)`, which short-circuits on first differing byte — a timing
side-channel on the console/serve key. Low practical risk for a 32-char random key
over a LAN, but trivial to fix: constant-time compare (e.g. fold XOR over equal-
length byte slices).

**B2 — Unbounded thread-per-connection.**
`http::serve` spawns one thread per accepted connection with no ceiling
(`http.rs:118-124`). A connection flood exhausts threads/memory. `MAX_BODY` and
`IO_TIMEOUT` exist, but concurrency is uncapped. *Fix:* bound in-flight connections
(a semaphore or a small fixed worker pool) and drop/503 past the cap.

**B3 — `/metrics` is unauthenticated on a LAN-bound console.**
`/metrics` is intentionally open like the probes (`app.rs:129-131`), but on the ISO
and container defaults the console binds `0.0.0.0`, so anyone on the network can
scrape model names, GPU models, and VRAM. *Fix:* gate `/metrics` behind the console
key, or make its exposure opt-in / loopback-only, keeping `/healthz`+`/readyz` open.

### C. Dead / drifting code

**C1 — `cameo-api` is an unused dependency; its transport is unimplemented.**
`cli/Cargo.toml` depends on `cameo-api`, but no `.rs` outside `core/api` references
it. The crate's own docs say the Unix-socket JSON-RPC transport "lands in Phase 2,"
and the README states "CLI + GUI bind to this" — which is not true today (both call
the core crates directly). *Fix:* remove the unused CLI dependency and soften the
README to "types define the contract; transport is Phase 2," or implement the
transport. At minimum, drop the dead dep.

**C2 — `Settings.model_dir` is dead.**
It is threaded through `overlay`/`resolve` (`config/src/lib.rs`) but never read: the
models crate uses the `CAMEO_MODELS_DIR` env, not this field. *Fix:* either delete
the field or make `models_dir()` honor it (which would also give a config-file way
to set the cache path, complementing A1).

**C3 — `core/containers` is linked by no binary.**
The passthrough recipe (189 loc + tests) is a library nobody consumes. *Fix:*
surface it (a `cameo containers …` helper or doc-generation) or label it explicitly
as a reference library so its "tested, unused" status is intentional, not drift.

### D. Repo hygiene / CI

**D1 — Repository URL is inconsistent.**
`Cargo.toml` and `containers/Containerfile` say `github.com/korbin/cameo`; the
systemd units say `github.com/sadlowskik/Cameo`. Pick one.

**D2 — The most dangerous shell scripts are unlinted.**
`ci.yml` shellchecks `scripts/*.sh`, `scripts/phase1/*.sh`, `containers/*.sh` — but
**not** `archiso/airootfs/usr/local/bin/*` (`cameo-install`, which wipes disks,
`cameo-firstboot`, `cameo-console-init`). *Fix:* add them to the shellcheck job.

### E. Product / roadmap gaps (acknowledged, but load-bearing)

**E1 — `/v1` gateway does not stream.** The proxy buffers the whole upstream
response (`proxy.rs`), so `stream: true` chat completions arrive as one blob at the
end — SSE clients see nothing until completion. Roadmap-noted; needs a streaming
passthrough path.

**E2 — Placeholder constants and flag names are unverified.** `bits_per_weight`,
`KV_BYTES_PER_LAYER_PER_TOKEN`, `TRAINING_FOOTPRINT_MULT`, and every llama.cpp /
PyTorch / `rpc-server` flag are best-effort placeholders pending a real Phase-1 run
(`model.rs`, `command.rs`, `net-strategy`). This is the single biggest unknown: the
planner's numbers and the actual spawn flags could be wrong on hardware.

**E3 — F2's live-medium "prompt once for a data partition" is not implemented.**
Only `cameo-install` (to disk) handles persistence; a running live USB has no
first-boot flow to point the cache at real storage. Related to A1.

**E4 — `/readyz` re-runs full detection per probe.** It shells out to
`lspci`/`rocminfo`/`rocm-smi` on every call (`app.rs:122-128` → `detect_report`).
Under a k8s liveness cadence that is repeated subprocess spawning. *Fix:* cache the
detection snapshot with a short TTL.

---

## Remediation plan (sequenced so each batch stays green and independent)

The ordering minimizes risk: pure removals first (cannot change behavior), then
isolated, unit-testable logic fixes, then the ops/config change validated across
all three delivery modes, then hardening, then the hardware-gated work last.

**Batch 1 — Pure cleanup, zero behavior change (land first).**
C1 (drop unused `cameo-api` dep + README wording), C2 (remove or wire
`Settings.model_dir`), D1 (single repo URL), D2 (shellcheck the airootfs scripts).
Exit: `fmt`/`clippy`/`test`/`shellcheck` all green; no runtime change.

**Batch 2 — Isolated correctness fixes, each with a unit test.**
A2 (port-collision check — a `Supervisor` test with two models on one port),
A3 (windowed restart cap — extend the existing `restart_decision` tests),
B1 (constant-time compare — a small equality test). These touch one module each
and add tests; no cross-crate change.

**Batch 3 — The models-dir/ops fix (A1), validated in all three modes.**
Remove the unit's `CAMEO_MODELS_DIR` override; add a `tmpfiles.d` (or ExecStartPre)
that creates `/var/lib/cameo/models`; confirm: (a) installed system writes there,
(b) container already correct, (c) live medium either persists or preflight-refuses.
Fold E3 in here if the live-persistence prompt is in scope. This is a config change
with no Rust logic change, but it needs the three-mode check, so it stands alone.

**Batch 4 — Hardening that needs care under load.**
B2 (bound connection concurrency), B3 (`/metrics` auth/opt-in), E4 (cache
detection for `/readyz`). Each changes daemon behavior under real traffic; land
separately so a regression is easy to bisect.

**Batch 5 — Larger / hardware-gated (do last, independently).**
E1 (streaming gateway — a real feature, own PR), E2 (Phase-1 calibration of
constants and flags — blocked on hardware; the code is already centralized for
exactly this), C3 (decide `core/containers`' fate once the container path is
exercised).

**Continuous:** keep CI green after each batch; the Phase-1 checklist
(`scripts/phase1/`) remains the gate for anything the CI runners cannot verify.
