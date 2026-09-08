# Knossos production completion plan

## Scope and evidence

This plan covers only `daedalus/knossos-rs` and its release-facing documentation. It is
an implementation plan, not release qualification. The 2026-09-05 audit is
correct that the locally passing 568-test Rust suite, revalidated 2026-09-07,
cannot qualify providers:
`daedalus/knossos-rs/tests/live_engine.rs` returns successfully without contacting a model when
`KNOSSOS_LIVE_OLLAMA` is absent (unless its strict opt-in flag is set).

The repository already has substantial foundations: `mission.rs` has an
append-only hash-chained journal and snapshots; `talos.rs` records intents,
results, compaction and Oracle verdicts; `context.rs`, `lethe.rs`, `delegate.rs`,
`mcp.rs`, `sandbox.rs`, and the engine adapters provide useful primitives. Those
facts do not prove that the product acceptance paths below are wired end to end.

## Dependency order and acceptance gates

| Issue | Concrete source evidence | Missing wiring / acceptance gate | Dependency and priority |
|---|---|---|---|
| KNS-LOOP-001 | `mission.rs::MissionContract`, `talos.rs::start_mission` | Operator amendments and a persisted contract shown before execution; restart preserves accepted contract. | P0 kernel root. |
| KNS-LOOP-002 | `gate.rs`, Talos approval hook | Execution modes must choose a bounded policy and show why an action is autonomous, grouped, or paused. | After 001. |
| KNS-LOOP-003 | `ariadne.rs`, `oracle/`, `Outcome` | Progress must be evidence/plan based; approvals and final result need a stable product object. | After 001–002. |
| KNS-LOOP-004 | `repl.rs`, `MissionPhase`, checkpoints | Accept/revise/revert must retain lineage and recover a previously durable mission. | After 001, 006. |
| KNS-LOOP-005 | `mnemosyne.rs`, `episode.rs` | Accepted capability memory needs scoped consent, expiry, and observable adoption metrics. | After policy and 002. |
| KNS-LOOP-006 | `MissionStore`, hash-chain snapshots | Version migration, startup recovery, and power-failure tests; no unknown schema may silently replay. | P0 foundation for 004/007/014. |
| KNS-LOOP-007 | `context.rs`, `lethe.rs`, `MissionEvent::Compacted` | Semantic capsule, artifact manifest, token accounting and invariant tests across repeated compaction. | After 006. |
| KNS-LOOP-008 | `engine/types.rs`, adapters | Provider-neutral durable action/result protocol and artifact rehydration across engines. | After 006–007. |
| KNS-LOOP-009 | `metis.rs`, Ariadne redirects | Durable DAG statuses, evidence-based replan and bounded recovery test matrix. | After 008. |
| KNS-LOOP-010 | `tools/mod.rs`, cancellation | Locks, idempotency keys and safe parallel reads must survive cancellation/restart. | After 008; before scheduler. |
| KNS-LOOP-011 | `ProofRecord`, Oracle invalidation | Content/revision-bound proof obligations, flaky handling and independent final gate. | After 008–010. |
| KNS-LOOP-012 | `delegate.rs` | Typed child contracts, ownership enforcement and isolated writer/reviewer fixtures. | After 010–011. |
| KNS-LOOP-013 | `config.rs`, engine adapters, budget wrapper | Capability negotiation, provider usage/cost reconciliation and strict per-provider limits. | After 008; live gates required. |
| KNS-LOOP-014 | journal replay, `tests/live_engine.rs` | Crash/replay/long-context/provider-switch matrix plus one cloud and one generic-local live conformance run. | Last kernel gate. |
| KNS-ENV-001 | Baseline: `sandbox.rs` only supplied child environment. Current slice adds `daedalus/knossos-rs/src/environment.rs` plus a durable mission record. | Execute approved setup/verification through a bounded adapter, retain resource/cache results, and qualify clean clones on supported platforms. | First E7 slice; it improves mission correctness without pre-empting the kernel dependencies. |
| KNS-GIT-001 | read-only git tool and `main.rs` revision helper | Dirty tracked/staged/untracked inventory, isolated worktree delivery, patch manifest and non-destructive restore fixture. | After ENV and action locks. |
| KNS-EXT-001 | `mcp.rs`, hooks, LSP | Versioned namespaced manifest, lazy health/circuit behavior, per-mission enablement and SDK conformance. | Before externally consequential integrations. |
| KNS-ID-001 | `sandbox.rs` strips credentials | Opaque broker, scoped TTL projection, revocation/rotation and canary-redaction path tests. | Before external actions. |
| KNS-TRUST-001 | Themis policy and hooks | Trust labels plus redirect/DNS-aware destination egress enforcement and hostile fixture suite. | Before browser/MCP network use. |
| KNS-SCHED-001 | single-session Talos; no durable queue | Ten-mission persistent queue with fair repository locks, leases, quota recovery and no duplicate external calls. | Depends on 006, 010, 013. |
| KNS-VIS-001 | editor pane only; no isolated evidence browser | Browser/a11y/network/screenshot artifact tool with hashes bound to build revision; visual required work cannot verify without it. | Depends on trust, identity and artifacts. |
| KNS-HAR-001 | Rust harness and Python reference coexist | Published parity matrix, decision record and supported-platform CI evidence. | After core conformance. |
| KNS-EVAL-001 | `eval.rs`, mock cases; optional live test | Hard suite must be non-saturated and separately report mock, live-local, live-cloud and Cameo outcomes. | After HAR and live conformance. |

## Current implementation slice: KNS-ENV-001

The first change is deliberately narrow. It does not claim container isolation,
install dependencies, or run discovered commands. It makes host execution
truthful and reproducible: discovery runs before Metis planning, stores a
bounded environment record plus hashes of discovered instruction/lock files in
the mission journal, blocks same-process and persisted resume when those files drift, records the
hash-bound result of a full Oracle verification, offers explicit setup and verification recipes,
and can export that record only when requested. A clean
clone still requires its declared setup command to be approved and executed by
the operator or a later capability-gated implementation.

The CLI task path captures this record before Metis planning. The serve, ACP,
and REPL first-task paths now do the same; `Talos::start_mission` retains an
idempotent fallback for direct callers and delegated children. `MissionStore::open`
provides read-only, integrity-checked historical access to every journal;
`MissionStore::open_for_resume` alone rechecks captured instruction, lock,
manifest, OS, and architecture facts before execution. Older journals without
an environment record remain inspectable but are refused for resume; operators
must create a revised mission after the new environment is captured. Full Oracle
verification similarly fails closed if that fingerprint drifted after capture.

Files for this slice:

- `daedalus/knossos-rs/src/environment.rs`: deterministic discovery and safe JSON export.
- `daedalus/knossos-rs/src/mission.rs`: persisted environment record/event with replay.
- `daedalus/knossos-rs/src/talos.rs`: attaches a pre-plan discovery result to each mission.
- `daedalus/knossos-rs/src/main.rs`: `task` performs discovery before planning and can export it.
- `daedalus/knossos-rs/src/lib.rs`, the environment unit tests, the mission-loop integration test, and this document.

Acceptance tests for the slice cover a Rust workspace with instructions and a
lockfile, bounded content hashes and drift detection, a non-Git directory,
deterministic ordering, recipe export, journal replay, and the task-path
ordering invariant. Discovery deliberately does not launch PATH tools or read
credentials; runtime probing needs a separately approved, bounded capability.
These tests do not count a missing live engine as qualification.

## E7 completion checklist

E7 closes only when the following product paths have evidence, not when helper
modules merely exist:

- Environment: clean-clone preparation, supported platform matrix, cache and
  resource limits, and exported setup/verification recipe.
- Git delivery: dirty tracked/staged/untracked preservation through worktree,
  reviewed patch and non-destructive restore; publishing is independently gated.
- Extensions: signed/versioned manifests, namespace/health/revocation behavior,
  lazy load, and read-only plus consequential SDK conformance fixtures.
- Identity and trust: opaque credentials, expiration/revocation/redaction, trust
  labels, redirect/DNS-aware egress policy, and hostile repository/tool fixtures.
- Scheduler: ten missions persist through restart with fair locks, quotas,
  cancellation races and no duplicate consequential call.
- Visual proof: isolated origin-scoped browser session, DOM/a11y/console/network
  reports and hash-bound captures; UI work cannot be `verified` without declared
  visual evidence.

## LOOP-004/006 recovery boundary

`knossos task`, `serve`, and `repl` expose `--persist-conversation` and
`--resume-mission`; ACP enables persistence at startup and accepts
`resumeMission` on `session/new`, where the editor supplies the workspace.
All routes restore through `MissionStore::open_for_resume`, refuse MCP,
delegation and dry-run restore, and recheck environment drift. Fresh-process
CLI, Serve and ACP tests preserve mission identity; the CLI test also proves
the accepted contract, execution-policy hash, accumulated quota and prior
conversation survive without replaying an earlier write. REPL uses the same
Talos restore path and has command-line wiring coverage.

This is a partial LOOP-004/006 closure, not release qualification. A complete
implementation still needs accept/revise/revert lineage, MCP/delegation and
preview capsules, provider capability state, unresolved-action reconciliation,
schema migration and power-failure coverage, plus live-provider recovery. It
must continue to refuse execution when a capsule is absent, an intent is
uncertain, or environment evidence has drifted; historical replay remains
read-only.

The Cameo adapter now applies advertised native-tool, context, completion-token,
and request-byte ceilings from v1 discovery instead of claiming generic OpenAI
tool support. Live pinned-backend conformance remains open.

## Evidence still required for release

Before a stable release, run the full Rust suite, format and strict Clippy;
then execute live conformance against a cloud provider, generic local server and
Cameo mock/endpoint with strict mode enabled. Platform, hardware, browser and
security qualification remain separate gates named in `PRODUCTIZATION_PLAN.md`.
