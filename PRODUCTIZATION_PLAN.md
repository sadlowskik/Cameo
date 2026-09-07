# Knossos + Cameo Productization Plan

Status: proposed execution plan  
Audience: product, engineering, design, release, and security  
Primary objective: turn Knossos and Cameo into two secure, truthful, installable, supportable products that work exceptionally well together without requiring each other.

## 1. Product definition

### Knossos

Knossos is the standalone agentic coding system. It owns:

- the Rust harness and its CLI, `serve`, and ACP interfaces;
- planning, retrieval, tool execution, containment, undo, delegation, and verification;
- the VS Code extension and editor-facing protocol support;
- Knossos Field: the local multi-agent command surface, including World, City, Senate, Routines, and Traces;
- provider adapters, including the Cameo engine adapter;
- harness contracts, conformance tests, and evaluation suites.

Knossos must work with cloud APIs, local OpenAI-compatible servers, and Cameo. A user must be able to install and use it without installing the Cameo operating system.

### Cameo

Cameo is the local-first AMD inference fabric and operating environment. One Cameo device is a complete inference appliance; trusted devices automatically become one elastic pool of models and available capacity. Cameo owns:

- GPU detection and compatibility reporting;
- local model installation, verification, placement, and lifecycle;
- supervised inference runtimes and one authenticated OpenAI-compatible serving endpoint;
- `cameod`, its control plane, request router, capacity scheduler, leases, secure pairing, and node coordination;
- the portable Cameo Link node/client needed to contribute compatible non-appliance systems to the local pool;
- the CLI, web console, container profile, live ISO, and installed system;
- hardware validation, recovery, update delivery, and appliance documentation;
- the versioned `cameo-engine` implementation consumed by Knossos.

Cameo must be valuable without Knossos: it should be a polished way to operate local models and expose one safe inference endpoint across one or many devices. Knossos is the flagship workload bundled with Cameo, not the only workload it can run.

### Shared product promise

The combined experience is:

1. Boot or install Cameo on one AMD system; it becomes a complete local inference node.
2. Cameo identifies the hardware, recommends a supported runtime, verifies a model, and exposes one authenticated local endpoint.
3. Additional devices are discovered, explicitly paired, and contribute only the capacity and models their owners allow.
4. Cameo routes each independent request to an eligible device while preserving session affinity, privacy, and visible placement.
5. The user launches Knossos Field; Knossos discovers the pool through the versioned engine contract and can spread independent agent/subagent calls across it.
6. Every action, route, permission, capacity decision, verification result, and rollback state is represented truthfully.
7. The user can reclaim a device, stop, recover, export, unpair, or uninstall without losing control of work or data.

## 2. Product and architecture decisions

These decisions should be accepted before implementation begins.

### 2.1 Rust is the supported Knossos runtime

The Rust harness becomes the product implementation for the CLI, ACP, editor integration, Field sessions, and Cameo integration. The Python implementation remains:

- a behavioral reference;
- an evaluation and research environment;
- a parity oracle for features not yet ported;
- a place to prototype changes before they are committed to the stable protocol.

New user-facing features should not ship in Python alone. A documented parity table will identify the remaining differences. Python is not removed until Rust passes the agreed parity, hard-suite, and live-provider gates.

### 2.2 Maintain two repositories

Use two public product repositories:

- `Knossos-Harness`: Rust harness, Python reference, Field, editor extension, contracts, conformance, evaluations, and release tooling.
- `Cameo`: daemon, core crates, CLI, web console, distro/ISO, containers, hardware data, and integration tests.

Field is imported at `Knossos-Harness/field/` and is owned by the Knossos repository. Do not create a third Field repository until its protocol and release cadence have demonstrably diverged from Knossos. Field has its own package, version, changelog, CI job, and downloadable artifact while remaining in the Knossos monorepo.

### 2.3 One versioned integration contract

Define `cameo-engine/v1` as the only boundary between the products. It must describe:

- capability discovery and protocol version negotiation;
- model inventory and health;
- pool identity, aggregate availability, and whether a request was routed locally or to a paired node;
- context window, request-size, native-tool, reasoning, and streaming capabilities;
- session create, heartbeat, lease, cancellation, and release;
- optional mission/session affinity, priority, deadline, privacy, and independence hints without exposing node addresses to the harness;
- inference requests and streaming responses;
- normalized authentication, rate-limit, capacity, model-missing, cancellation, and server errors;
- request/job correlation IDs and truthful selected-node/fallback metadata;
- usage and cost telemetry;
- privacy flags identifying whether data stays local;
- forward-compatible optional fields and explicit behavior for unknown fields.

The contract needs a JSON Schema, example transcripts, a compatibility policy, and a mock server used by both repositories in CI.

### 2.4 Field is an operator interface, not a security boundary

Prompts and visual labels are not enforcement. All permissions, scopes, budgets, workspace boundaries, and campaign rules must be enforced in server-side policy code or in the harness itself. The UI explains policy decisions; it does not create them.

### 2.5 Truth is a product feature

Every operational fact must carry a source:

- `observed`: emitted by a real harness, filesystem watcher, provider, or operating system;
- `derived`: deterministically computed from observed facts;
- `manual`: asserted by the operator;
- `synthetic`: generated by a rehearsal or fixture.

Synthetic and manual events must never silently raise production verification, maturity, cost, or reliability scores.

### 2.6 Knossos schedules work; Cameo schedules compute

Knossos decides how a mission is decomposed, which agent calls are independent, and what model capabilities/privacy the work requires. Cameo decides which trusted node should execute each inference request based on model presence, observed capacity, health, affinity, owner policy, and network cost.

Knossos may provide optional hints such as mission/session ID, independent-call group, priority, deadline, latency/quality preference, and local-only privacy. Cameo treats them as bounded scheduling inputs, not permission to violate node policy. Knossos never depends on device addresses, and Cameo never changes mission semantics or decomposes agent work. Clients that provide no hints still receive correct routing through the ordinary OpenAI-compatible endpoint.

## 3. Release gates

No public preview should be published until all P0 gates pass. No stable release should be published until every P1 gate passes.

### P0: public-preview blockers

- Authenticated HTTP and WebSocket access to Field.
- Strict origin and host validation; no wildcard CORS.
- Markdown XSS removed and a restrictive CSP enabled.
- Filesystem and terminal symlink/junction escapes prevented.
- Sensitive environment variables minimized for every child process.
- Tool, read, write, environment, budget, concurrency, and delegation policies enforced rather than prompted.
- Production and synthetic state separated.
- Field imported into the Knossos repository and covered by CI.
- Licenses and asset provenance present.
- Release artifacts built from a clean, tagged commit.

### P1: stable-release blockers

- Rust/Python parity decision completed and documented.
- Hard evaluation suite produces useful, non-saturated results.
- Live provider conformance passes for at least one cloud and one local engine.
- Long missions pass semantic compaction, process-kill resume, provider-switch, cancellation, and duplicate-side-effect tests.
- Reproducible environment setup and dirty-worktree preservation pass on every supported platform.
- Prompt-injection, secret-broker, destination-aware egress, and third-party tool trust suites pass.
- Git/patch delivery and browser-based visual verification pass their end-to-end fixtures.
- Cameo mock integration suite and real AMD hardware matrix pass.
- Cameo's public claims are generated from one capability manifest and every `certified` combination has attached release evidence.
- Cameo passes black-box `/v1` streaming/cancellation/error conformance, durable supervisor recovery, model-provenance, and offline-operation suites.
- On each certified hardware class, a clean first boot can recommend, transactionally install, configure, and prove a supported runtime/model profile without undocumented tuning.
- Cameo Mesh passes discovery/pairing, mTLS, three-node heterogeneous routing, owner-reclaim, node-loss, session-affinity, and truthful-placement gates.
- A Cameo update interrupted at every defined boundary either completes or boots the prior healthy system without losing models or state.
- ISO build, boot, install, persistence, upgrade, and recovery pass.
- WCAG 2.2 AA audit passes for operator-critical workflows.
- Performance budgets pass on the supported desktop baseline.
- SBOMs, vulnerability scans, signed checksums, and release provenance are published.
- Upgrade and rollback paths are tested from the previous supported version.

## 4. Workstream A — Field security boundary

### A1. Local authentication and session bootstrap

Implement a default-secure local session model:

1. Generate a cryptographically random 256-bit Field control secret at startup.
2. Persist it with owner-only permissions when persistence is required; otherwise keep it process-local.
3. Add a one-time bootstrap URL that exchanges the secret for an `HttpOnly`, `SameSite=Strict` session cookie and immediately redirects to a clean URL.
4. Require the authenticated cookie on every HTTP API route and WebSocket upgrade.
5. Give each harness permission bridge a separate, short-lived, session-scoped capability token. It must not use the browser control credential.
6. Rotate browser sessions when Field restarts and provide an explicit `knossos field logout`/revoke action.
7. Continue binding to loopback by default. Remote access must require an explicit configuration with TLS or a documented secure tunnel.

Acceptance tests:

- unauthenticated API requests return `401`;
- foreign origins cannot read state or submit commands;
- foreign-origin WebSockets are rejected before a snapshot is sent;
- expired, replayed, and malformed bootstrap secrets fail;
- a permission token can decide only its own session's request;
- restart invalidates prior ephemeral browser sessions.

### A2. Browser request protections

- Remove wildcard CORS. Same-origin requests should need no CORS headers.
- Validate `Host` against the configured loopback host and port.
- Reject missing or foreign `Origin` on state-changing browser requests and all WebSocket upgrades.
- Use a CSRF token for any deployment mode that cannot rely exclusively on strict same-site cookies.
- Return uniform JSON errors without stack traces, paths, secrets, or child-process details.
- Apply the existing body-size cap to every route, including routine toggles.
- Add request deadlines and connection limits.

### A3. Content security

- Replace the custom Markdown-to-HTML implementation with a maintained parser plus a strict sanitizer.
- Allow only the Markdown elements actually needed by Field.
- Permit only safe URL schemes (`http`, `https`, and intentionally supported local anchors).
- Add a CSP beginning with `default-src 'self'`, no inline scripts, and `frame-ancestors 'none'`.
- Add `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, and an explicit permissions policy.
- Restrict the browser pane to validated `http`/`https` URLs.
- Remove `allow-same-origin` from sandboxed external content unless a documented workflow requires it.
- Open untrusted external sites in a separate, isolated surface where practical.

Security fixtures must include malicious repository names, Markdown, links, SVG, filenames, prompts, event payloads, and terminal output.

### A4. Filesystem containment

- Canonicalize workspace roots once at mount time.
- For reads, require the canonical target to remain beneath the canonical root.
- For writes to new files, canonicalize the nearest existing parent and reject symlink/junction traversal.
- Use no-follow semantics where the platform supports them.
- Apply the same containment helper to file tree, file read/write, Git operations, terminal working directories, adaptive sizing, and watcher attribution.
- Reject device files, named pipes, sockets, and unsupported reparse points.
- Add Windows junction and Unix symlink regression tests.
- Respect `.gitignore` plus an explicit sensitive-name denylist in City.
- Hide `.env*`, credentials, private keys, package-manager credentials, cloud configuration, `.git`, Field state, and user-configurable secret patterns by default.

### A5. Terminal and process safety

- Require a validated workspace-relative working directory.
- Add configurable process timeout, output cap, line cap, and concurrent terminal cap.
- Kill process trees, not only the direct shell process.
- Apply WebSocket backpressure limits and disconnect clients whose buffers exceed the bound.
- Do not persist raw commands containing recognized credentials.
- Display the exact workspace and command before approval.
- Separate inspection commands from mutation commands with a real parser/allowlist where policy depends on the distinction; do not rely on a mutation regex as the final boundary.

### A6. Child environment minimization

- Build child environments from a documented allowlist rather than copying the entire parent environment.
- Pass only the provider credential required by the chosen endpoint.
- Scrub cloud, CI, package registry, SSH-agent, desktop-session, and unrelated provider credentials.
- Ensure agent-run subprocesses inherit the harness-scrubbed environment.
- Add canary-secret tests that attempt to read and echo unrelated environment variables through every adapter.

Deliverable: `docs/threat-model.md`, including assets, trust boundaries, attack paths, mitigations, accepted risks, and security-contact instructions.

## 5. Workstream B — Policy and resource enforcement

### B1. Unified policy object

Create one normalized policy passed to both the Anthropic and Knossos adapters:

- allowed tools;
- denied tools;
- allowed read globs;
- allowed write globs;
- workspace and optional target-subtree boundary;
- environment scope;
- maximum cost;
- maximum turns and wall-clock duration;
- delegation depth and child count;
- network policy and allowed domains;
- whether terminal execution is permitted;
- whether operator approval is required for each consequence class.

Adapters may implement policy differently, but the conformance tests must prove equivalent outcomes.

### B2. Enforce role scopes

- Architect may write only approved Markdown paths.
- Archivist may write only Field memory paths.
- Challenger, Scout, and Verifier receive a true read-only tool registry.
- Read access is also workspace-bound; read-only must not mean machine-readable.
- Per-agent loadouts may narrow but never widen the role.
- Campaign environment scopes apply to every tool path, subprocess, network call, and adapter.

### B3. Budget enforcement

Implement four independent ceilings:

- per-turn token/request ceiling;
- per-session dollar ceiling;
- per-routine-run dollar ceiling;
- aggregate campaign dollar ceiling.

Requirements:

- use provider-reported usage where available and a clearly labeled estimate otherwise;
- reserve budget before spawning concurrent work so several agents cannot overshoot simultaneously;
- stop at the nearest safe boundary and emit a durable `budget.exhausted` event;
- distinguish `zero cost`, `unknown cost`, and `missing telemetry`;
- show spent, reserved, remaining, and estimation source;
- test replay, restart, provider retry, and duplicate usage events.

### B4. Concurrency and delegation

- Enforce campaign concurrency during mobilize, reinforce, assignment, resume, and automated routine spawns.
- Enforce global and per-endpoint session caps.
- Enforce delegation depth and child count in the harness, not only in the system prompt.
- Ensure child sessions inherit a narrower or equal policy.
- Emit explicit events for denied, queued, admitted, and released capacity.

## 6. Workstream C — Truthful gameplay and event integrity

### C1. Replace heuristic verification

Define the four World metrics precisely:

- Activity: recent observed work; never implies completion.
- Completion: all declared definition-of-done items have evidence.
- Verification: an independent verifier/referee recorded a passing verdict against that evidence.
- Persistence: the verified result still exists in the current workspace revision or promoted checkpoint.

Maturity is computed from explicit events only. No session role, `done` state, file count, folder discovery, or fallback constant may manufacture verification. The UI should expose the events behind every percentage.

### C2. Separate rehearsals

- Put rehearsal sessions, assignments, filesystem events, and scores in a synthetic namespace.
- Add a clear persistent visual treatment while a rehearsal is active.
- Rehearsals may demonstrate growth, but the growth disappears or remains in a separate demo history when the run ends.
- Never mix rehearsal results into normal Traces, project maturity, costs, or campaign evidence without an explicit filter.
- Correct all copy to say “synthetic events using the production event schema.”
- Offer a deterministic reset and a completion summary.

### C3. Complete lifecycle semantics

- Emit `assignment.completed` when all assigned sessions complete successfully.
- Emit `assignment.failed`, `assignment.cancelled`, or `assignment.interrupted` for the other terminal outcomes.
- Derive assignment state from member state on replay so missing terminal events can be repaired deterministically.
- Expire or archive old assignments in the default Traces view without deleting history.
- Add search, status filters, source filters, grouping, and expandable/copyable event payloads.

### C4. Real checkpoint and rollback behavior

Choose and label one of two modes:

- Record-only: checkpoint and rollback are governance facts; UI says “mark promoted” and “record rollback.”
- Managed Git: Field creates a named Git reference or worktree checkpoint and can generate a reviewed restore operation.

For a polished coding product, implement Managed Git with explicit confirmation, clean/dirty-tree handling, conflict reporting, and no destructive reset. The failed history remains append-only even when files are restored.

### C5. Event-log durability

- Add schema versioning and migrations.
- Run an integrity check at startup and expose degraded/recovery state.
- Treat malformed JSONL anywhere except an incomplete final record as corruption, not a line to silently skip.
- Add configurable backups and a documented restore command.
- Compact or snapshot projections without deleting the canonical audit trail.
- Bound `/api/events`, campaign trace, and replay requests with validated pagination.
- Avoid scanning the entire log for every campaign trace.
- Test power loss, torn writes, duplicate events, clock anomalies, corrupt rows, migration rollback, and million-event replay.

## 7. Workstream D — Routines

### D1. Durable runtime state

- Fold `routine.enabled` events during startup so toggles survive restarts.
- Disabling a routine cancels pending debounced triggers.
- Expose last run, next eligible run, last outcome, and current owner.
- Add an explicit run history separate from agent assignments.

### D2. Scheduling semantics

- Adopt a maintained cron parser or fully document the supported subset.
- Validate schedules at config load instead of silently accepting inert expressions.
- Define timezone explicitly.
- Match standard day-of-month/day-of-week semantics.
- Prevent duplicate firing after clock jumps or process pauses.
- Add overlap policies: `skip`, `queue-one`, `parallel`, or `replace`.
- Add cooldowns and maximum queued runs.

### D3. Routine safety

- Apply the routine's own budget and policy.
- Validate role, endpoint, workspace, trigger, and completion configuration before startup.
- Require operator confirmation before enabling a routine that can mutate files or use the network.
- Detect self-trigger loops caused by a routine changing files matched by its own trigger.
- Add deterministic tests for debounce, disable-during-debounce, restart, overlap, DST, malformed cron, budget stop, and self-trigger prevention.

## 8. Workstream E — Knossos harness product

### E1. Make the agent loop the primary product experience

Knossos should make delegation feel faster and safer than opening a general-purpose chat and supervising it manually. Its default product loop is:

```text
Throw task → mission contract → meaningful progress → independent proof → accept, revise, or revert
```

The user should experience one task, one evolving status, and one proven result. Plans, tool calls, traces, model routing, and multi-agent orchestration remain inspectable, but they are supporting machinery rather than the default interface.

#### Mission contract

Within a few seconds of receiving a task, Knossos produces a compact, executable contract:

```text
Mission     Add resumable model downloads
Scope       Four files in cameod/model/
Risk        Medium — persistence and partial files
Strategy    Inspect → implement → interrupt test → verify
Budget      Up to $1.20 / 12 minutes
Safety      Isolated worktree; network disabled
Proof       Restart test + checksum test + existing suite
Unknowns    Windows cancellation behavior may require a separate gate
```

The contract is not a speculative essay. It is the durable agreement used by execution, verification, cost enforcement, and the final result. It contains:

- the requested outcome;
- explicit in-scope and out-of-scope boundaries;
- risk class;
- execution mode;
- workspace isolation strategy;
- tool and network policy;
- time, token, and dollar ceilings;
- definition of done;
- concrete verification commands or evidence requirements;
- unresolved assumptions that could materially change the work.

The default actions are:

- **Dispatch:** proceed autonomously inside the contract.
- **Guide:** proceed, but stop at meaningful product checkpoints.
- **Campaign:** use blue, red, and independent referee orchestration.

The user can edit the contract before dispatch. Once work begins, material scope expansion requires a visible amendment; the harness cannot silently redefine success.

#### Progressive autonomy router

Knossos automatically chooses the lightest loop capable of producing trustworthy evidence:

| Task class | Default loop |
|---|---|
| Explanation, location, or diagnosis | Scout only; no mutation |
| Small reversible edit | Builder + targeted verification |
| Normal feature or bug fix | Planner → Builder → Oracle |
| Security, migration, or release work | Blue → Red → Referee |
| Broad ambiguous objective | One focused question → mission contract |

Routing is based on measured scope, affected interfaces, declared environment, expected reversibility, and risk signals—not keywords alone. The selected loop and reason are visible. Users may escalate, but a low-risk task should not require configuring an operation, doctrine, and five agents.

#### Agent execution kernel

The interface above is only credible if Knossos has a durable execution kernel underneath it. The target is not to imitate another harness's chat UI. It is to make long-running work coherent, resumable, economical, steerable, and provable even when the model makes mistakes or the context window turns over.

Current Knossos has valuable pieces already: Talos owns the turn loop, Metis emits a bounded plan, Ariadne applies pressure and detects repeated no-progress calls, Oracle establishes a pre-change baseline and verifies independently, Lethe bounds context, ToolCtx journals writes and checkpoints, Interjections allow live steering, Resilient retries transient engine failures, and one-level delegation isolates a child's transcript. Keep those foundations.

The product gaps are structural:

| Area | Current behavior | Product target |
|---|---|---|
| Durable state | The transcript and a small episodic record carry most intent | A typed, versioned `MissionState` is the source of truth |
| Compaction | Rust preserves tool pairing by eliding the middle of large text/result blocks | Semantic state compaction with exact invariants, trace references, and a deterministic emergency fallback |
| Planning | A short linear list is divided into turn slices | A dependency-aware plan that is revised from evidence without narrating every trivial action |
| Tool loop | Calls are validated and permissioned, then generally executed in sequence | Capability-aware scheduling: parallel safe reads, serialized conflicting writes, normalized results, cancellation, and idempotency metadata |
| Progress | File changes and repeated call signatures are the primary signals | Progress is measured against contract criteria, evidence gained, blockers removed, and workspace deltas |
| Delegation | A one-level child shares the workspace and returns a summary | Typed child contracts and evidence handoffs, explicit file ownership, isolated writers, and independent review roles |
| Recovery | Turn/attempt checkpoints and traces exist | Atomic mission snapshots, replayable journals, provider-independent resume, and external-change reconciliation |

##### Canonical mission state

Create a versioned `MissionState` stored independently from provider messages:

```text
MissionState
  identity       mission_id, parent_id, revision, created_at, last_safe_checkpoint
  contract       outcome, scope, exclusions, constraints, risk, definition_of_done
  policy         execution_mode, tool grants, network/secret rules, approval leases
  budget         token, request, time, dollar, tool, child, and retry limits + actuals
  plan           nodes, dependencies, status, owner, expected output, proof, rollback
  focus          phase, active node, current hypothesis, next action, stop reason
  knowledge      observed facts, assumptions, decisions, contradictions, open questions
  evidence       reproductions, command results, diagnostics, tests, reviews, provenance
  workspace      root identity, base revision, changed paths, journal, locks, snapshots
  failures       attempts, error families, forbidden retries, recovery actions
  children       contracts, budgets, ownership, status, structured handoffs
  verification   baseline, required checks, completed checks, gaps, final verdict
  conversation   provider cursor, recent message range, compaction generation
```

Every fact in `knowledge` is typed as `observed`, `inferred`, `user_supplied`, or `accepted_project_memory`; records include provenance and freshness. An assumption can guide work but cannot satisfy a proof requirement. A changed file is not evidence of completion. Provider transcripts are disposable projections of this state, not the only place the state exists.

Persist `MissionState` after every consequential tool result, approval decision, plan revision, compaction, child handoff, and verifier verdict. Use an atomic append-only journal plus periodic snapshots. Version the schema and include migrations before calling resume stable.

##### Loop state machine

Implement the outer lifecycle as an explicit, event-sourced state machine:

```text
INTAKE -> RECON -> CONTRACT -> BASELINE -> PLAN -> EXECUTE
                                          ^        |
                                          |        v
                                      REPLAN <- EVALUATE
                                                   |
                                                   v
CHALLENGE -> VERIFY -> HANDOFF -> ACCEPTED
     ^          |          |          |
     |          v          v          v
     +------ REPLAN      REVISE     REVERTED

Any active state -> PAUSED | BLOCKED | CANCELLED | FAILED | RECOVERING
```

State transitions must have typed causes and guards. `VERIFY -> HANDOFF(verified)` requires all contract proof obligations to pass against the current workspace revision. A user interjection may amend the contract, reprioritize the plan, pause, cancel, or answer a question; it cannot silently bypass policy or convert missing proof into success.

Within `EXECUTE`, each turn follows one deterministic supervisor cycle:

1. Reconcile cancellation, interjections, external file changes, expired approvals, and child events.
2. Evaluate hard budgets and policy before spending another model request.
3. Select the active plan node and its unresolved completion criterion.
4. Retrieve only fresh code, instructions, history, and evidence relevant to that node.
5. Compile a provider request from the stable prefix, state capsule, retrieval packet, and recent verbatim exchange.
6. Ask the model for a typed action envelope: `act`, `answer`, `ask`, `delegate`, `verify`, `replan`, `pause`, or `finish`.
7. Validate the envelope, tool schemas, paths, capabilities, budgets, dependencies, and permission leases without trusting model prose.
8. Schedule independent read-only calls concurrently when safe; serialize writes and any calls with overlapping resources or unknown side effects.
9. Normalize every result into `observations`, `artifacts`, `workspace_delta`, `diagnostics`, `cost`, `retryability`, and `provenance`.
10. Refresh changed index entries and invalidate stale retrieval packets, diagnostics, and prior verification tied to older content hashes.
11. Update `MissionState`, append trace events, and create a checkpoint for consequential progress.
12. Score progress, detect contradiction or stagnation, and choose continue, challenge, verify, replan, recover, ask, or halt.
13. Compact or rotate provider state when thresholds are reached, then validate the rebuilt context before the next request.

The supervisor, not the model, owns transitions, permissions, budgets, completion, retry limits, and the final verdict.

##### Action and result protocol

Define a provider-neutral action envelope so engines cannot encode control flow only in prose:

```json
{
  "intent": "act",
  "plan_node": "download.resume-test",
  "rationale": "The reproduction now isolates partial-file handling.",
  "calls": [{"tool": "edit", "input": {}, "reads": [], "writes": []}],
  "expected_observation": "Interrupted downloads retain a valid partial file.",
  "on_failure": "inspect cancellation path",
  "confidence": 0.74
}
```

Adapters may translate native tool calls into this envelope, but the kernel validates it identically for every provider. Tool definitions declare side effects, resource patterns, network/secret use, expected duration, idempotency, cancellation support, retry safety, output limits, and whether calls can run in parallel. Unknown metadata defaults to the safer behavior.

Store full raw outputs in the trace/artifact store and feed the model a bounded normalized view. Large results return a summary plus stable artifact ID, content hash, line/range metadata, and a retrieval handle. The agent can rehydrate exact details later instead of paying to resend them every turn.

##### Context compiler and compaction

Build each request from ordered layers:

1. stable constitution, security policy, role, and compact tool catalogue;
2. mission contract and immutable user constraints;
3. latest validated state capsule;
4. active plan node and proof obligation;
5. fresh repository/retrieval packets with source hashes;
6. recent verbatim user, assistant, tool-use, and tool-result groups;
7. the newest interjection or supervisor instruction last.

Keep stable material byte-identical and first so provider prompt caches can reuse it. Put volatile status, retrieval, and the immediate instruction later. Use provider tokenizers when available; maintain calibrated per-provider estimates with a safety margin otherwise. Record estimated and provider-reported usage so the estimator can be tested.

Use four context tiers:

- **Verbatim window:** current exchange, unresolved errors, pending approvals, and recent tool-call/result groups.
- **State capsule:** a compact semantic rendering of `MissionState`, regenerated rather than recursively summarized.
- **Artifact store:** exact full outputs, patches, logs, and prior message ranges addressable by ID and hash.
- **Durable project memory:** accepted, source-backed facts that survive missions and can expire.

Compaction policy:

- At 65% of the usable context window, pre-compute a new state capsule and identify oversized artifacts.
- At 78%, rotate completed exchanges out of the verbatim window after their observations and evidence are committed.
- At 90%, use an emergency deterministic compactor that preserves the mission, constraints, active node, unresolved blockers, exact identifiers, latest errors, pending permissions, changed paths, verification gaps, and next action.
- Compact proactively at phase boundaries and before an expected high-output operation; do not wait for a provider rejection.
- Prefer provider-native compaction or continuation cursors when available, but always retain the portable Knossos state capsule and trace so a mission can switch engines or resume offline.
- Never split or orphan a tool-use/tool-result pair, recursively summarize an old summary, discard an unresolved contradiction, or erase the fact that content was omitted.

Every compaction emits a manifest containing generation, source event range, state version, retained message groups, artifact references, token counts, and hashes. A deterministic validator checks the rebuilt context against `MissionState`. If required fields or call/result pairing are missing, reject that compaction and fall back to the deterministic capsule. Add a canary asking the next model turn to identify the mission, active step, blockers, changed files, and next proof; disagreement triggers recovery rather than continued execution.

##### Planning, retrieval, and replanning

Replace a purely linear plan with a small DAG. Each node has dependencies, owner, risk, inputs, expected artifact, completion predicate, verification method, budget slice, and rollback point. Keep plans coarse: model-internal micro-actions do not need separate nodes.

Planning is optional for a trivial reversible task, just-in-time for discovered complexity, and mandatory for cross-component, destructive, migration, security, or release work. Reconnaissance must establish repository instructions and the cheapest useful baseline before mutation.

Replan when evidence invalidates an assumption, a dependency changes, the same error family survives two materially different attempts, scope expands, a required tool is unavailable, budget burn diverges, external edits conflict, or verification exposes a new failure. Replanning keeps completed evidence and successful work; it does not restart the mission or quietly rewrite the contract.

Retrieval packets carry path, symbol/range, content hash, index revision, why the item was selected, and freshness. Refresh changed files immediately. Separate repository instructions and user-authored policy from similarity-ranked code so retrieved source can never masquerade as an instruction. Measure retrieval precision on the hard suite and penalize irrelevant context, stale snippets, missed governing instructions, and repeated reads with no new evidence.

##### Progress, stuck recovery, and stopping

Calculate progress from contract criteria satisfied, new evidence, blockers resolved, plan nodes completed, diagnostics reduced, and meaningful workspace deltas. Tool activity alone is not progress.

Classify failures as transient provider/network, tool/schema, permission, environment, deterministic code/test, flaky verification, context corruption, policy violation, or unknown. Recovery follows a bounded ladder:

1. retry a declared retry-safe transient failure with jitter and a deadline;
2. repair malformed input once using the tool schema and error;
3. inspect fresh state or use an alternate tool;
4. test a materially different hypothesis;
5. rewind to the last safe checkpoint if the workspace is worse;
6. replan from preserved evidence;
7. delegate a bounded investigation or independent review;
8. ask one focused user question when authority or missing information is truly required;
9. halt `blocked` with exact evidence, attempted recoveries, and the smallest useful next action.

Detect stagnation using repeated call signatures, unchanged content hashes, repeated diagnostic families, no new evidence, circular plan transitions, and low information gain. Do not let model confidence reset the detector. Completion is permitted only when the contract is satisfied and Oracle's required tiers pass; budget exhaustion, a plausible answer, or a model saying "done" are distinct non-success outcomes.

##### Verification as part of the loop

Turn the definition of done into executable proof obligations at contract time. Capture pre-existing failures before edits. During work, run the cheapest targeted check after each meaningful change; before handoff, run the required ladder against the exact current workspace revision:

```text
reproduce -> syntax/type/static -> targeted tests -> affected suite
          -> integration/system -> adversarial review -> contract judge
```

Each result records command/tool identity, environment, start/end time, exit status, output artifact, changed-source hashes, and whether it was baseline, worker-supplied, or independently produced. A later write invalidates dependent proofs. Detect flaky checks through bounded reruns and report uncertainty rather than selecting the favorable run. If a required environment is unavailable, return `partially_verified` or `blocked`, never `verified`.

##### Delegation and multi-agent work

A child receives a typed sub-contract, relevant state capsule, explicit files/resources, tool policy, budget, required handoff evidence, and no unrelated parent transcript. It returns a structured handoff: findings, changes, exact evidence/artifact IDs, unresolved risks, cost, and suggested next action.

Read-only investigators and reviewers may share a workspace snapshot. Writers should use isolated worktrees or exclusive path ownership; shared-write mode remains an explicit constrained optimization, not the default. The scheduler rejects overlapping write sets, detects undeclared writes, and never grants a child broader permissions than its parent. Reviewer/referee roles remain unable to edit what they judge. Child success is provisional until the parent reconciles the workspace and independent verification passes.

Start with one level of delegation and bounded concurrency. Increase either only after evals prove better accepted-result latency and cost without reducing reliability. More agents are not inherently a better loop.

##### Provider, cost, and effort routing

Add capability negotiation for structured outputs, native tool calls, parallel calls, context size, token counting, continuation cursors, native compaction, streaming, reasoning controls, cancellation, cached input reporting, and usage/cost fields. A provider adapter declares capabilities; the kernel chooses supported behavior or a tested fallback rather than scattering provider checks through Talos.

Route effort per phase: a fast inexpensive model for classification, retrieval triage, and routine transforms; a stronger model for ambiguous planning, difficult implementation, recovery, or contract judging. Escalate only on measured need. Preserve the same `MissionState` when switching models. Enforce request, token, wall-clock, dollar, tool, retry, and child budgets in the supervisor and include cache reads/writes in accounting.

##### Persistence, steering, and observability

- Support pause, resume, cancel, amend, reprioritize, and "do not try that again" at the next safe boundary; interrupt cancellable tools immediately when requested.
- On restart, compare workspace identity, base revision, journal hashes, active child leases, and last snapshot before resuming. Reconcile or branch when external changes are present.
- Redact secrets at event creation, not only in the UI. Raw trace access is local and permissioned; exported support bundles exclude source and prompts by default.
- Give every request, tool call, artifact, checkpoint, plan node, child, approval, compaction, and verdict stable correlation IDs.
- Record why the supervisor chose a transition, retry, model, tool, replan, compaction, or halt. Keep hidden reasoning private; expose auditable decisions and evidence.
- Make traces replayable with mocked providers/tools so loop regressions can be reproduced deterministically.

##### Kernel invariants and eval gates

The kernel is release-ready only when automated tests prove:

- a 100+ turn synthetic mission compacts repeatedly without losing the original outcome, current plan node, exact IDs, pending approvals, blockers, changed paths, or tool pairing;
- the process can be killed after any journaled event and resume to the same safe state without duplicate consequential calls;
- switching providers immediately after compaction preserves mission behavior and evidence references;
- a write invalidates every proof tied to the prior content hash;
- parallel reads reduce latency, while overlapping writes remain serialized or rejected;
- cancelled and timed-out calls cannot later write into the workspace unnoticed;
- transient failures retry within policy, deterministic failures do not burn the retry budget, and circuit breaking works;
- three different stuck patterns trigger bounded recovery and terminate without loops;
- interjections during planning, tool execution, compaction, verification, and delegation produce deterministic safe outcomes;
- child permissions, budgets, depth, and write ownership can never exceed the parent contract;
- pre-existing failures are distinguished from regressions and missing checks cannot produce `verified`;
- the same recorded mission replays to equivalent state transitions across supported engines;
- hard-suite tasks measure accepted-result rate, regression rate, intervention count, context-loss errors, resume success, p50/p95 latency, cache utilization, and cost—not just whether the model eventually emitted an answer.

Implement this kernel in vertical slices: first `MissionState` and journal; then the context compiler and semantic compactor; typed action/result envelopes; progress/recovery; plan DAG; provider capabilities; safe parallel scheduler; structured delegation; and finally deterministic replay plus long-horizon evals. Keep the existing Lethe block elision as the emergency structural fallback until the semantic path passes pairing and recovery tests.

#### Execution rhythm

The agent loop has seven user-visible phases:

1. **Acknowledge:** restate the outcome and begin bounded reconnaissance.
2. **Contract:** present scope, risk, budget, safety, and proof.
3. **Prepare:** create the worktree/snapshot and establish the verification baseline.
4. **Execute:** perform work with grouped, consequence-aware approvals.
5. **Challenge:** test the riskiest assumption or original reproduction.
6. **Verify:** independently reproduce the contract's definition of done.
7. **Handoff:** present a result object with accept, revise, and revert actions.

Progress updates describe meaningful state transitions, not raw tool noise:

```text
✓ Located the download lifecycle
✓ Established failing interruption test
✓ Added partial-file recovery
→ Interrupting a real fixture download to test resume
```

Raw events remain in Traces. The normal status surface should answer what changed, what is happening now, what is blocked, what it costs, and what evidence remains.

#### Consequence-aware approvals

- Reads and explicitly safe diagnostics inside the contract proceed without interruption.
- Related writes are grouped into one reviewed change intent where possible.
- Network transmission, secret access, external publication, destructive actions, production mutation, and scope expansion remain individually visible.
- Permission requests show action, target, consequence, originating mission step, and safer alternatives.
- Repeated identical approvals can be granted for the mission scope only; they do not become global permanent permissions accidentally.
- Permission spam is tracked as a product defect.

#### Result object

Every mission ends in a structured result rather than an open-ended final chat message:

```text
VERIFIED

Changed       4 files
Proved        Resume, checksum, partial cleanup
Checks        38 passed
Cost          $0.74 actual
Duration      8m 12s
Unverified    Windows cancellation behavior

[Review diff] [Accept] [Revise] [Revert]
```

Possible top-level outcomes are `verified`, `partially_verified`, `rejected`, `blocked`, `cancelled`, and `failed`. A mission cannot report `verified` when required checks were skipped, unavailable, or inferred from the worker's own claim.

Accept records the verified capability and its evidence. Revise creates an amendment that preserves the workspace, trace, prior evidence, and failed approaches. Revert performs the defined safe rollback and retains the complete audit history.

#### Continuation and recovery

- Follow-ups such as “make it work on Windows too” continue the same mission lineage.
- Resume restores contract, workspace, evidence, budget state, and last safe checkpoint.
- A restarted harness distinguishes resumable, interrupted, externally modified, and unrecoverable work.
- Concurrent external edits trigger reconciliation instead of silent overwrite.
- The user can export a self-contained mission report for review or support.

#### Compounding project knowledge

Accepted missions may add narrowly scoped durable knowledge:

- verification commands that actually passed;
- directory and ownership boundaries;
- project-specific setup requirements;
- confirmed architecture decisions;
- recurring failure modes;
- the user's mission-level approval preferences;
- measured model performance for a task class.

Conversation is not memory by default. Every proposed durable fact shows its source and can be accepted, edited, rejected, expired, or deleted. Generated knowledge never overrides repository instructions or current observed state.

#### Field integration

Field visualizes the mission loop truthfully:

- reconnaissance opens a workfront;
- execution builds visible but unverified work;
- challenge introduces findings or confirms the risky path;
- independent proof earns verification;
- acceptance promotes a capability and grows the settlement;
- rejection or rollback leaves visible history rather than erasing the attempt.

The completion moment should provide a concise ceremony: outcome, evidence, cost, changed surface, remaining uncertainty, and the next useful action. Settlement growth is the visual history of accepted verified capabilities, not a reward for tool activity.

#### Adoption metrics

Optimize the loop for:

- time to first useful action;
- time to an accepted mission contract;
- time to independently verified result;
- percentage of missions completed without intervention;
- verification pass and rejection rates;
- permission prompts per mission;
- actual cost per accepted result;
- rollback and resume success;
- percentage of first-time users who dispatch a second mission;
- percentage of users who inspect evidence before accepting.

Do not optimize for agent count, tool-call count, token consumption, or visual activity. Telemetry must be opt-in or local-first, documented, and must not collect source, prompts, diffs, credentials, or terminal content by default.

Agent-loop acceptance criteria:

- a new user can dispatch a small safe change within five minutes of installation;
- the first useful action begins within ten seconds on the supported baseline, excluding engine cold start;
- a normal low-risk change needs no orchestration configuration;
- skipped proof cannot produce a verified result;
- every write is recoverable through the mission's isolation/rollback path;
- a follow-up preserves mission context without replaying the entire setup conversation;
- user tests show the primary experience feels like delegating to a trusted engineering team rather than operating an agent debugger.

### E2. Resolve implementation parity

Create `docs/parity.md` with one row per capability and these columns:

- Python status;
- Rust status;
- conformance test;
- supported providers;
- behavior differences;
- decision: port, retain as research-only, or remove.

Close product-critical Rust gaps first: multi-language verification, provider compatibility, cancellation, permission parity, delegation enforcement, context compaction, usage telemetry, MCP/LSP behavior, and recovery.

### E3. Establish stable interfaces

- Version ACP extensions, NDJSON `serve`, trace schema, and Field event schema.
- Restore or remove stale compatibility aliases deliberately.
- Generate protocol fixtures from schemas.
- Ensure older clients receive actionable version errors.
- Publish a support matrix for editor, OS, architecture, engine, and protocol versions.

### E4. Improve CLI onboarding

Target workflow:

```text
knossos doctor
knossos init
knossos task "add the feature"
knossos field
```

Requirements:

- `doctor` checks binary dependencies, credentials without printing them, engine reachability, workspace permissions, and verifier toolchains;
- `init` creates an explicit project configuration and does not overwrite files;
- first run explains execute versus dry-run mode;
- failures identify the failed boundary and the next corrective command;
- Ctrl+C, cancellation, resume, and undo are reliable;
- configuration precedence and environment variables are documented in one place.

### E5. Evaluation quality

- Run and publish the hard suite against representative local and cloud models.
- Add tasks that measure honesty, rollback, permission refusal, stale-context recovery, concurrent edits, prompt injection, partial verification, and cost discipline.
- Prevent benchmark contamination by versioning fixtures and keeping hidden held-out cases.
- Report success, honesty, cost, latency, tool count, retries, and verification strength separately.
- A missing verifier toolchain is `skipped`/`unverified`, never a pass.

### E6. Editor extension

- Package the VSIX in every Knossos release.
- Add first-run binary discovery/install guidance.
- Make permission requests, plans, tool calls, diffs, verification, cancellation, and reconnect states first-class.
- Test workspace trust, remote workspaces, multi-root workspaces, large diffs, extension reload, and protocol mismatch.
- Meet keyboard navigation, screen reader, high contrast, and reduced-motion requirements.

### E7. Complete the surrounding work loop

The cognitive kernel is necessary but not sufficient. Users leave a harness when it can reason about a change but cannot reliably prepare the project, access the right tool, inspect the running product, or deliver the finished work. Knossos must own these boundaries without silently acquiring broader authority.

#### Reproducible mission environments

- Discover repository instructions, toolchain declarations, lockfiles, workspace topology, and supported setup commands before planning.
- Fingerprint OS, architecture, runtime/compiler versions, package managers, lockfiles, environment policy, and relevant service dependencies in the mission baseline.
- Support host, container, and user-supplied environment adapters behind one contract. Never imply isolation when running directly on the host.
- Make dependency installation and network access explicit contract capabilities. Show packages, source registry, expected writes, and cache impact before approval.
- Reuse content-addressed dependency/build caches without sharing secrets or mutable workspaces between missions.
- Apply disk, process, port, CPU, memory, and lifetime limits. Clean abandoned environments through a recoverable TTL policy.
- Record the exact setup and verification recipe in the result so another machine or CI can reproduce it.
- Distinguish repository failure, missing host dependency, unsupported platform, and agent regression rather than collapsing them into a generic tool error.

#### Git-native delivery

- Detect clean, dirty, untracked, detached, nested, submodule, LFS, and non-Git workspaces before mutation.
- Default mutating missions to a named worktree/branch when Git is available; preserve and clearly attribute pre-existing user changes.
- Offer reviewable patch, selected-hunk apply, mission commit, branch, and pull-request-ready handoff modes.
- Generate commit/PR text from the verified contract and evidence, not from an unverified model summary.
- Treat commit, rebase, merge, push, tag, release, PR creation, and review submission as separate consequential capabilities. Local edits never imply permission to publish.
- Integrate CI status as additional evidence while keeping the local verifier result distinct. A green remote job cannot prove it tested the current unpushed workspace.
- Detect upstream movement and conflicts before delivery; reconcile visibly or create a new branch instead of force-pushing or discarding work.
- Export a portable patch plus mission/evidence manifest when no forge integration is configured.

#### Tool, MCP, and extension platform

- Provide one namespaced capability registry for built-ins, MCP servers, LSPs, project skills, and future plugins.
- Lazily search/load large tool catalogues so every prompt does not pay for every schema.
- Require each integration to declare version, source, permissions, side effects, data destinations, secret needs, cancellation, idempotency, retry behavior, resource locks, and output limits.
- Pin integration versions per project or installation and show compatibility errors before a mission spends model calls.
- Add health checks, timeouts, circuit breakers, revocation, and per-mission enable/disable controls. A failing optional server must not break the entire harness.
- Namespaces prevent tool collisions; tool results retain server identity and provenance through compaction and export.
- Ship an extension SDK, manifest schema, conformance suite, example read-only tool, and example consequential tool.
- Keep install and marketplace discovery separate from runtime authorization. Installing an integration does not grant it workspace, network, or secret access.

#### Secret and identity broker

- Store credentials outside prompts, traces, mission state, project files, and child environments. Refer to them with opaque handles.
- Mint the narrowest short-lived credential or environment projection a specific approved tool call can use; revoke it on completion, cancellation, timeout, or mission end.
- Scope credentials by provider/service, account, operation, destination, workspace, child, and expiration where the upstream system permits.
- Never expose a secret value to the model merely because a command needs authenticated execution.
- Show which identity/account an external action will use before approval and in the resulting audit event.
- Detect and redact canary secrets in prompts, tool output, logs, patches, browser surfaces, crash reports, and exports. Block transmission when safe redaction cannot preserve the action.
- Support credential rotation and disconnected/expired states without corrupting the mission; resume from the blocked capability after re-authentication.

#### Untrusted-content and exfiltration boundary

- Encode instruction precedence in the supervisor: product policy and current user authority outrank repository instructions; repository instructions outrank ordinary source, web pages, issue text, logs, and tool output.
- Label retrieved content by source and trust class. Content may provide data but cannot grant itself tools, network, secrets, broader scope, or permission.
- Treat source comments, test fixtures, generated files, web pages, terminal output, model output, and MCP responses as potentially adversarial.
- Apply destination-aware egress rules to network tools. Deny loopback services, cloud metadata endpoints, private networks, and undeclared hosts by default when browsing untrusted content.
- Require an explicit data-flow preview before sending source, diffs, logs, prompts, or artifacts to a new external destination.
- Sanitize rendered Markdown/HTML, terminal escape sequences, links, filenames, and downloaded artifacts before displaying or opening them.
- Add adversarial fixtures for indirect prompt injection, poisoned repository instructions, malicious tool descriptions/results, encoded secret requests, SSRF, DNS rebinding, terminal control sequences, and cross-child data access.

#### Multi-mission scheduler

- Maintain a durable queue with priority, fairness, per-repository write exclusion, global provider/tool limits, Cameo GPU capacity, and user-defined cost ceilings.
- Allow concurrent read-only missions and isolated worktree writers; prevent two missions from mutating the same checkout or claiming the same port/resource.
- Support pause, resume, reorder, cancel, retry-from-checkpoint, and replace semantics without duplicating consequential calls.
- Reclaim expired child/tool leases after crashes and surface orphaned processes, locks, ports, and worktrees in `knossos doctor`.
- Coalesce notifications and interrupt the user only for approval, actionable blockage, meaningful completion, or failure unless verbose updates are requested.
- Expose why a mission is queued: repository lock, provider rate limit, budget, dependency, hardware capacity, or operator pause.
- Test starvation, priority inversion, restart, cancellation races, lock leakage, quota exhaustion, and concurrent external edits.

#### Browser, visual, and artifact verification

- Add an isolated browser tool that can inspect DOM, accessibility tree, console, network failures, screenshots, responsive breakpoints, and authenticated local preview sessions.
- Use temporary browser profiles and origin-scoped session material. Downloads, clipboard, file uploads, popups, external navigation, and localhost access follow mission policy.
- Let proof obligations include screenshot comparison, layout bounds, keyboard flow, accessibility checks, console cleanliness, and Core Web Vitals where relevant.
- Store visual captures and reports as hashed evidence artifacts tied to the source/build revision that produced them.
- Support explicit artifact viewers for images, video, audio, PDFs, logs, and generated documents without injecting their embedded content as trusted instructions.
- Never claim a visual or interaction requirement passed solely because code compiled or a model described the expected screen.

Surrounding-loop acceptance criteria:

- a clean clone can be prepared and its verified setup recipe exported without undocumented manual intervention;
- a dirty repository mission preserves every pre-existing user change through execute, revert, and delivery;
- an agent can prepare a patch or PR-ready branch, but cannot publish anything without the corresponding explicit capability;
- optional tools can be installed, inspected, pinned, scoped, disabled, and removed without changing core mission semantics;
- no tested prompt-injection path can grant authority, reveal a canary secret, or create an undeclared network destination;
- ten queued missions survive restart with correct locks, budgets, priorities, and no duplicate side effects;
- a frontend mission requiring visual proof cannot end `verified` until the declared browser and accessibility evidence exists.

## 9. Workstream F — Field experience and visual system

### F1. Naming contract

Use this vocabulary everywhere:

- Product: Knossos Field.
- Primary sections: Field, Routines, Traces.
- Field lenses: World, City, Senate.
- World contains projects, workfronts, gateways, and agents.
- Senate contains operations, teams, findings, verdicts, checkpoints, promotions, and rollbacks.

Update UI, routes, README, screenshots, shortcuts, event descriptions, prompts, test fixtures, and old Cameo/Daedalus references in one atomic naming pass. Keep legacy executable and schema aliases only where compatibility requires them, and label them deprecated.

### F2. World gameplay

- Add collision-aware unit placement, clustering, or formation stacking beyond six agents.
- Make selection, multi-selection, focus, and active fronts readable without hovering.
- Add operation-complete, failed, interrupted, and attention feedback.
- Preserve earned city state only when backed by production verification events.
- Let users inspect exactly why a settlement grew or regressed.
- Keep the roster usable with 30 agents and no horizontal micro-text wall.
- Provide filters for team, role, state, project, and campaign.

### F3. Rome and Atlas themes

Rome remains the expressive default. Atlas becomes a genuinely useful technical theme rather than a dimmed Rome skin:

- increase terrain and label contrast;
- add distinct project, front, gateway, model, storage, and verification landmarks;
- give the capital a clear technical anchor;
- retain readable labels at normal zoom;
- use shared information hierarchy and interaction behavior across both themes;
- test every state in both themes, including empty, crowded, error, selected, focused, and rehearsal.

### F4. Responsive strategy

Do not compress the entire desktop RTS interface into a phone. Define:

- Desktop (`>= 1100px`): full command surface.
- Compact/tablet (`760–1099px`): reduced panels with full operational control.
- Mobile (`< 760px`): status, approval, alerts, agent detail, pause/cancel, and trace review; map command gestures are secondary.

Actionable text should normally be at least 12px on desktop and 14px on mobile. Touch targets should be at least 44px. The complete navigation must not remain squeezed into one row on narrow screens.

### F5. Accessibility

- Meet WCAG 2.2 AA for all release-critical workflows.
- Add visible focus, logical focus order, skip navigation, landmarks, and descriptive live regions.
- Include the routine name in toggle labels.
- Ensure every graphical metric has equivalent text and provenance.
- Test modals, menus, file tree, editor, terminal, replay slider, campaign forms, and map regions with keyboard only.
- Test at 200% zoom, high contrast, reduced motion, and screen-reader browse/forms modes.
- Use automated accessibility scanning plus a manual checklist; neither substitutes for the other.

### F6. Product states and onboarding

- Create a guided first launch: choose workspace, choose engine, run doctor, select capital, start a safe rehearsal, then start real work.
- Clearly distinguish disconnected, unauthenticated, no-engine, engine-down, permission-waiting, budget-exhausted, interrupted, and corrupted-state screens.
- Provide a sample repository and deterministic walkthrough.
- Add an in-product command reference and searchable help.
- Make all destructive or expensive actions previewable.

## 10. Workstream G — Assets and web performance

### G1. Asset cleanup

- Remove unused capital variants from the shipped public directory; keep source iterations under `design/archive` or external design storage.
- Add `ASSET_PROVENANCE.md` with asset name, creator, source/tool, creation date, transformations, license, attribution requirement, and review status.
- Document trademark and historical-map review.
- Use Git LFS for large design sources if they remain in the repository.
- Add a CI check that reports unreferenced public assets and unexpected bundle growth.

### G2. Image delivery

- Encode the Mediterranean map as responsive AVIF/WebP with a PNG fallback only if required.
- Provide at least compact and high-density sizes.
- Lazy-load settlement modules by theme and visibility rather than preloading all four unconditionally.
- Preserve transparent edges and test compositing on dark and light diagnostics backgrounds.
- Self-host the selected fonts or use a deliberate system-font stack for offline operation.

### G3. Static serving

- Brotli/gzip JavaScript, CSS, SVG, and JSON where beneficial.
- Use immutable caching for fingerprinted assets and revalidation for HTML.
- Add ETags or content hashes.
- Return correct content types and security headers on every response.
- Avoid loading City, Senate, editor, and rehearsal-only code before needed.

### G4. Performance budgets

Initial proposed budgets, adjusted only with measured justification:

- initial compressed transfer: <= 2.5 MB including the visible map;
- JavaScript: <= 150 KB gzip for initial World load;
- CSS: <= 50 KB gzip;
- LCP: <= 2.5 seconds on the agreed throttled desktop profile;
- CLS: <= 0.1;
- interaction latency: <= 100 ms for ordinary selection and navigation;
- sustained 30-agent update stream without dropped controls or unbounded memory growth;
- one slow WebSocket client cannot raise server memory without bound.

Record Lighthouse/Core Web Vitals traces in CI where stable and run a real-browser performance profile before each release candidate.

## 11. Workstream H — Cameo appliance quality

### H1. Freeze the v1 product contract

Cameo cannot be validated while its own documents describe different products. The stable v1 contract should be:

> Cameo turns one or more trusted devices into a private AMD-first inference fabric. Every node can serve verified models itself; together they expose one authenticated OpenAI-compatible endpoint that routes independent requests to the best currently available capacity for people, applications, and Knossos.

Recommended scope classification:

| Capability | Stable v1 status |
|---|---|
| AMD single-node inference over validated Vulkan/ROCm combinations | Required |
| CPU fallback for diagnosis and minimal operation | Supported fallback, not a performance promise |
| Browser console, CLI, model lifecycle, authenticated `/v1`, updates, recovery | Required |
| Knossos engine integration | Required |
| Hardware-aware recommended model/runtime setup with one-action install and proof | Required |
| Automatic discovery, secure pairing, live capacity/model advertisements, and request-level routing | Required |
| One local OpenAI-compatible endpoint across the trusted pool | Required |
| Elastic join/leave, foreground-device protection, session affinity, and visible job placement | Required |
| Portable Cameo Link for supported non-appliance nodes and clients | Required on the published v1 platform matrix |
| Quantization | Preview until quality, disk, cancellation, and thermal gates pass |
| Distributed model execution | Experimental and off by default |
| NVIDIA/Intel acceleration | Container experiment; outside the AMD appliance support promise |
| Training and custom MoE expert placement | Research/post-v1 until end-to-end hardware evidence exists |
| Kubernetes and multi-tenant serving | Post-v1 |

Request-level pooling and distributed model execution are separate capabilities. The stable mesh sends each independent request to one eligible node and keeps it there for the request lifetime. It does not claim to merge VRAM or split an in-flight request. Cross-node model sharding remains experimental until its own performance, correctness, and failure-recovery gates pass.

Replace “any AMD card” with evidence-backed support language. Publish four labels: `certified`, `compatible`, `experimental`, and `unsupported`. “Certified” means the exact hardware/runtime/model path passed the release matrix; “compatible” means a defined family passed and the detected combination is within its supported bounds; “experimental” is best effort with explicit escape hatches; “unsupported” fails safely with an actionable report. Vulkan is the broad fallback, not a guarantee that every historical AMD card, firmware, kernel, and model combination works.

Create one canonical `cameo-capabilities.json` generated into the README, site, console, CLI, ISO metadata, and Knossos engine descriptor. It declares delivery mode, hardware status, backends, model formats, API features, context/streaming/tool-call support, mesh status, and known limitations. CI fails when public claims diverge from it.

Delete or reconcile the conflicting v1 definitions in `README.md`, `CAMEO_PROJECT_PLAN.md`, `docs/product-spec.md`, `docs/remediation-plan.md`, `docs/definition-of-done.md`, the website, and release metadata before the next public artifact.

### H2. Control-plane hardening

- Re-run the current source-level security audit; do not rely on stale line references in earlier reports.
- Test backend bearer propagation and node-push authentication with a mock `llama-server` and mock node.
- Centralize constant-time credential comparison.
- Keep open inference disabled by default.
- Refuse non-loopback binds without an explicit key and operator acknowledgment.
- Strictly enforce Unix socket owner and mode on every boot.
- Bind node callbacks to registered identities and protect node-push against DNS rebinding and internal-address abuse.
- Add rate limits, connection limits, request deadlines, body caps, and structured audit events.
- Move slow endpoint health probes outside supervisor-wide locks.

### H3. Model lifecycle

- Provide resumable model downloads with checksums and disk-space preflight.
- Record model source, license, quantization, size, compatibility, and integrity.
- Make load, unload, eviction, lease, failure, and recovery visible.
- Distinguish unsupported hardware, insufficient VRAM, unavailable runtime, and model incompatibility.
- Never report a model ready until the inference endpoint passes a real request.
- Add safe cancellation and cleanup for interrupted downloads and starts.

### H4. Hardware validation matrix

Create a versioned matrix covering representative:

- APU-only systems;
- discrete GPU systems;
- mixed APU + dGPU systems;
- supported ROCm path;
- Vulkan-only/lite path;
- constrained-memory and eviction scenarios;
- cold boot, warm boot, suspend/resume where supported, and driver failure.

For each machine record hardware identifiers, firmware, kernel, Mesa/ROCm/runtime versions, model, quantization, throughput, memory use, thermals if available, known limitations, and exact pass/fail steps. Generate `known-good-combo.json` from validated facts rather than guesses.

### H5. ISO and installed-system experience

Test both full and lite editions through:

1. reproducible ISO build;
2. signature/checksum verification;
3. UEFI and BIOS boot where supported;
4. hardware report and first-run message;
5. authenticated console access;
6. offline disk installation;
7. reboot into the installed system;
8. persistence of account, config, models, and logs;
9. update installation;
10. failed-update rollback;
11. factory reset/export without accidental model deletion.

Remove development-only defaults from installed systems. The live environment and installed environment must have separate, documented security postures.

### H6. Cameo UX

The Cameo console should make six questions obvious:

- What hardware did Cameo find?
- What can this machine run safely?
- What complete setup does Cameo recommend for my workload, and why?
- What is running now?
- Who is using it and what capacity is reserved?
- What should I do when something fails?

#### Hardware-aware first-run setup

The default first-run path is:

```text
Detect hardware -> choose workload -> compare recommendations -> approve
                -> install verified runtime -> acquire verified model
                -> configure -> smoke-test -> expose endpoint
```

For a supported machine, Cameo recommends a complete `RuntimeProfile`, not just a model name:

```text
backend/runtime     Vulkan, ROCm, or CPU + exact validated build
model               immutable digest, source, license, architecture, quantization
memory              GPU layers/split, KV type, context, batch, concurrency, headroom
behavior            chat template, tool/structured-output support, stop behavior
operations          expected download/disk use, load time, TTFT/tok-s range, power profile
evidence            hardware/runtime combinations that produced the estimate
```

- Ask the user to choose the intended workload: Knossos coding, general assistant, fast chat, long context, or custom. Workload changes the quality/context/latency tradeoff.
- Present three useful choices by default: `Recommended`, `Faster/lighter`, and `More capable`, plus an advanced override. Explain quality, context, speed, memory, disk, power, and support status in plain language.
- Base recommendations on the versioned hardware matrix and observed free resources. Mark extrapolated combinations `experimental`; never present an untested estimate as known-good.
- Account for display-reserved VRAM, unified-memory limits, other processes, thermal/power profile, desired concurrency, context/KV growth, and model/runtime feature compatibility.
- Consider the entire Cameo Mesh. Recommend a paired warm node when it produces a materially better supported experience; show whether the model will run here, elsewhere, or be mirrored.
- Before approval, show exact downloads, publishers, licenses, digests, total and temporary disk use, network destinations, configuration changes, expected time, and rollback behavior.
- Run installation as a journaled transaction: fetch and verify the pinned runtime, stage and verify the model, write configuration atomically, start under the supervisor, send a real representative inference/tool-format probe, and promote only after it passes.
- On failure, clean partials, restore the previous profile and endpoint, retain diagnostic evidence, and offer the smallest corrective action. A failed “one-click” setup must not leave a half-updated runtime as the default.
- Save the resulting profile so CLI, console, Knossos, restart recovery, mesh advertisement, and support bundles all report the same configuration.
- Keep an offline path: the ISO's starter runtime/model profile completes the same proof without internet; signed import bundles can add larger profiles later.

Expose the same workflow as `cameo setup`, `cameo recommend --workload knossos`, and machine-readable `--json`/API operations. “One click” describes the happy path, not hidden consent: licenses, large downloads, new network destinations, and destructive storage changes remain explicit.

Add a single `cameo doctor` report suitable for support, with explicit redaction and an export preview. Use consistent names across CLI, web console, service units, environment variables, docs, and boot messages.

### H7. Knossos bundle

- Bundle a tested Knossos version in the Cameo ISO while keeping it independently upgradeable.
- Launch Field through a Cameo command that performs authentication bootstrap safely.
- Prefer local Unix-socket communication between Knossos and `cameod` on the appliance.
- Mint a scoped Cameo token for Knossos; do not reuse the Cameo administrator credential.
- Show “local through Cameo” as a verifiable routing fact in Field.
- Test Cameo engine restart, model eviction, capacity refusal, SSE interruption, and appliance reboot while a Knossos session exists.

### H8. OpenAI compatibility and serving quality

Treat “OpenAI-compatible” as a versioned conformance claim, not shorthand for accepting one chat request.

Use a replaceable data-plane architecture:

```text
client / Knossos
       |
       v
local Cameo gateway  ->  auth, limits, request identity, streaming
       |
       v
Mesh scheduler       ->  eligible node + admission lease
       |
       v
node ingress         ->  cancellation, accounting, owner policy
       |
       v
runtime adapter      ->  managed llama.cpp first; Ollama/LM Studio where supported
       |
       v
model runtime        ->  Vulkan / ROCm / CPU execution
```

`cameod` owns supervision, routing, and policy, but it does not reimplement tensor kernels or model execution. The managed, pinned `llama-server` path is the reference runtime and the only one required for an appliance to be self-contained. External-runtime adapters let Cameo Link contribute existing Ollama or LM Studio installations after protocol conformance and capability probing.

- Publish the exact supported endpoint and feature matrix: model listing, chat completions, legacy completions if retained, streaming, usage, stop sequences, sampling fields, seeds, response formats, tool calls, embeddings, image/multimodal input, logprobs, cancellation, and error shapes.
- Forward only capabilities the selected backend/model actually supports. Reject unsupported parameters explicitly rather than ignoring them or returning plausible but incorrect output.
- Normalize model IDs, context limits, tokenizer/chat-template selection, finish reasons, usage counts, request IDs, HTTP statuses, and streaming termination across backend versions.
- Bound request bodies, generated tokens, context size, output buffering, queue length, concurrent streams, idle connections, and per-client work.
- Propagate disconnect and cancellation to `llama-server`; prove cancelled work releases queue, KV cache, memory reservations, and session leases.
- Add admission classes for interactive, Knossos, batch, and administrative work with documented fairness and priority. Prevent one long request or client from starving health checks and other users.
- Return retryable versus terminal errors, retry-after guidance, capacity state, and safe operator actions in machine-readable form.
- Version and run a black-box conformance suite against Cameo, a pinned `llama-server`, and the Knossos adapter for buffered and streaming requests.
- Bind every response and metric sample to node, endpoint, model digest, runtime build, backend, and request ID without exposing prompts by default.

Stable v1 need not implement every OpenAI feature. It must be precise about the subset it supports and remain compatible across patch releases.

### H9. Durable supervisor and failure recovery

The current in-memory endpoint/session/lease view must become recoverable appliance state.

- Persist desired endpoint state, model digest, backend, sanitized launch configuration, assigned devices, capacity reservation, restart history, leases, and last health result in a versioned local database.
- On daemon start, inventory child processes, listening ports, GPU allocations, model files, and persisted intents. Adopt only processes with verifiable Cameo identity; terminate or report ambiguous orphans rather than double-starting them.
- Separate desired, observed, and last-known-good state. A stale database row cannot make a dead endpoint appear ready.
- Use transactional state transitions for ensure/start/ready/lease/release/stop/evict. A crash at any transition must converge safely after restart.
- Window restart limits by time and reset only after sustained health. Preserve the terminal failure reason and remediation instead of flapping forever.
- Handle backend crash, daemon crash, power loss, kernel GPU reset, device disappearance, OOM kill, disk full, corrupt database, occupied port, and partial model deletion.
- Reconcile capacity from observed device/process memory after failures; never trust only the reservation ledger.
- Drain new work before planned updates or shutdown, enforce a deadline, then checkpoint or fail active requests explicitly.
- Add backup, integrity check, migration, and last-known-good restore for the state database.

### H10. Model catalogue and supply chain

Every local model is a managed artifact with identity and policy:

```text
ModelRecord
  digest, filename, size, format, architecture, parameter count, quantization
  source URL/repository/revision, publisher, license, acceptance requirement
  tokenizer, chat template, context limits, modality, backend compatibility
  acquired_at, verified_at, integrity status, local aliases, benchmark evidence
```

- Use content-addressed storage internally; aliases and display names point to immutable digests.
- Download into a quarantine/partial area, resume safely, verify expected size and SHA-256 or stronger digest, inspect GGUF with strict size/depth limits, then atomically promote.
- Do not run a model whose integrity check, format inspection, license decision, or required authentication is incomplete.
- Support authenticated/gated downloads through opaque credential handles; never store registry tokens in model metadata or command history.
- Preserve source revision and license text needed for redistribution. The bundled starter model requires a release-time license and redistribution audit.
- Resolve tokenizer and chat template explicitly. Detect incompatible or absent templates before serving instead of producing silently degraded conversations.
- Track which runtime/backend versions validated each digest. Runtime updates invalidate compatibility evidence until smoke-tested.
- Make delete and garbage collection lease-aware, crash-safe, and undoable until final reclamation. Never remove the only verified copy during an update or rollback window.
- Support offline import/export bundles containing the model, manifest, digest, license, and optional benchmark record.
- Treat conversion and quantization as new derived artifacts with parent digest, tool version, arguments, logs, quality checks, and atomic output.

### H11. Installer, storage, and data ownership

- Address disks by stable hardware identity, capacity, and partition map. Re-read the target immediately before destructive operations and refuse if it is the boot source, mounted elsewhere, changed, or ambiguous.
- Show an explicit final storage plan listing partitions, filesystems, encryption, bootloader, persistent model location, preserved data, and everything that will be erased.
- Support guided whole-disk install plus a documented advanced/manual path. Do not claim safe dual boot until it has dedicated fixtures and recovery documentation.
- Offer full-disk encryption for installed systems and explain the unattended-boot tradeoff. Protect API keys, prompts, traces, and model access metadata at rest.
- Make installation resumable only across proven-safe boundaries; otherwise restart cleanly and explain what was written.
- Separate OS, mutable configuration/state, logs, model storage, and user exports so OS rollback does not roll back or delete models accidentally.
- Apply quotas and reserve emergency free space for logs, state migration, and rollback. Refuse downloads/quantization before they consume that reserve.
- Define backup/export and restore for configuration, identities, model manifests, Cameo state, and optional models. Test restore onto replacement hardware.
- Factory reset offers separate choices for configuration/state, credentials, logs, and model data, with an exact preview and irreversible confirmation.
- Test NVMe, SATA, USB install media, unusual sector sizes where supported, interrupted partitioning, read-only/full filesystems, corrupt boot entries, and removable persistent-data disks.

### H12. Transactional updates and offline lifecycle

- Publish a signed component manifest covering the OS snapshot, kernel, firmware, Mesa/Vulkan, ROCm, `llama.cpp`, Cameo binaries, Knossos bundle, starter model, schemas, and compatibility IDs.
- Preflight disk space, power, active work, state/database migration, hardware support, model compatibility, and rollback availability before applying an update.
- Prefer A/B system images or another atomic system transaction for the appliance. If installed v1 uses pinned `pacman` snapshots, define and test the exact failure boundary, bootable last-known-good path, and package/database rollback.
- Drain endpoints, snapshot state, apply, reboot when required, run hardware/model/API health probes, and commit the update only after success. Automatically return to the prior bootable version on failure.
- Keep configuration and state schema migration forward- and rollback-aware. Refuse downgrade when restoration would lose data.
- Provide signed offline update bundles and model bundles. Core installation, first boot, local inference, diagnostics, rollback, and factory reset must work without internet or external DNS.
- Never silently update the kernel, GPU stack, inference runtime, models, or Knossos. Show compatibility impact and allow a maintenance window.
- Define channels with promotion evidence: nightly -> alpha -> beta -> release candidate -> stable. Hardware results attach to the exact component manifest.
- Exercise update from every supported prior version, interrupted download, interrupted apply, failed reboot, failed health commit, expired signing key, corrupted bundle, and rollback with preserved models.

### H13. Cameo Mesh — automatic local inference pooling

Cameo Mesh is a core product capability. It gives applications one local endpoint while trusted devices contribute models and available inference capacity behind it. This is request-level elasticity: one independent request runs start-to-finish on one selected node. It complements, but does not depend on, experimental cross-node model sharding.

#### Node roles and installation

- A Cameo appliance runs the complete node: detection, managed runtime, model catalogue, scheduler/router, console, and serving endpoint.
- A portable `cameo-link` package for supported Windows, macOS, and Linux systems contributes an existing Cameo runtime or a compatible OpenAI/Ollama/LM Studio endpoint through a constrained adapter.
- A client-only installation exposes the local proxy and UI but contributes no compute. This lets a laptop use the pool without hosting a model.
- Nodes advertise observed capabilities, never marketing guesses. A node is eligible only after its engine and exact model digest pass a real local health/inference probe.

#### Discovery and pairing

- Discover candidates with mDNS on trusted local interfaces and allow manual address entry for segmented networks. Discovery creates no trust and grants no traffic.
- Pair through an explicit short-lived code plus displayed device names and certificate fingerprints. Block node-to-node communication until both sides confirm.
- Generate persistent node identities and use mutual TLS for control, capacity advertisements, model operations, and proxied inference.
- Separate node, operator, client, and Knossos credentials. Support rotation, revocation, expiration, leave, remote removal, and complete reset.
- Restrict discovery and serving by interface/firewall profile. Never bridge guest Wi-Fi, VPN, container, or public interfaces automatically.

#### Live capacity advertisement

Each node publishes a signed, sequenced, short-lived `NodeSnapshot`:

```text
identity/version     node ID, certificate, protocol/runtime versions
models               immutable digests, aliases, templates, context/capabilities
compute              devices, backend, usable VRAM/RAM, measured performance class
pressure             queue, active jobs, KV/cache use, GPU/CPU utilization, thermals
availability         owner policy, foreground reserve, quiet hours, battery/power state
network              observed RTT/bandwidth class and last successful probe
health               engine/model readiness, degraded reasons, snapshot expiry
```

Advertisements expire quickly and are not capacity reservations. Stale, unsigned, incompatible, thermally degraded, sleeping, owner-reclaimed, or disconnected nodes are removed from eligibility without blocking healthy local operation.

#### One compatible serving endpoint

- Expose one loopback OpenAI-compatible endpoint for applications and Knossos, with an explicitly enabled authenticated LAN endpoint when desired.
- Preserve ordinary streaming semantics, cancellation, usage, errors, and request IDs through the proxy. The client does not need to know which node ran the request.
- Keep routing visible through response metadata/headers, traces, Jobs, and metrics: selected node, model digest, reason, queue time, first-token time, and fallback history.
- Provide compatibility adapters only for protocols with conformance tests. Do not silently reinterpret unsupported fields.
- Continue serving locally if the mesh controller or every remote node disappears.

#### Scheduler and admission algorithm

For each request, the router:

1. derives hard requirements: model digest/alias, modality, context, tool/structured-output features, privacy class, backend policy, and deadline;
2. filters to paired fresh nodes whose runtime, model, health, policy, memory, and feature capabilities satisfy every hard requirement;
3. estimates queue delay, cold-load/transfer cost, prefill/decode time, network cost, and failure risk;
4. scores warm model residency, free capacity, session/KV affinity, measured throughput, current load, foreground-device reserve, thermals, battery/power policy, and user preference;
5. obtains a short admission lease from the chosen node before accepting the route;
6. proxies and streams the request while renewing the job lease and propagating cancellation;
7. records observed timing and memory so later estimates improve without collecting prompt content.

Prefer a slightly slower warm/affine node over thrashing models between devices. Spread truly independent Knossos child/referee calls across nodes, but keep sequential turns on the same warm endpoint when that reduces KV/prefix cost. Let users pin a mission, model, or client to a node and reserve foreground capacity for gaming or creative work.

#### Failure and retry semantics

- A request never migrates after generation begins. Before the first response byte, a retry-safe request may be re-admitted elsewhere within its deadline and retry budget.
- After partial streaming, node loss returns an explicit interrupted error with request/job identity; Cameo never splices a second model's continuation into the stream.
- Client disconnect cancels remote generation and releases its admission/session lease.
- Node sleep, owner reclaim, thermal pressure, engine stop, and network loss stop new admissions immediately while bounded in-flight work drains or fails visibly.
- Reservations expire after controller failure. Reconciliation prevents duplicate active jobs and double-accounted capacity after restart or partition healing.

#### Model placement and replication

- Support `local-only`, `pin`, `mirror`, `on-demand`, and `never-copy` policies per model and node.
- Route to an existing warm copy by default. Suggest or perform an approved resumable copy only when predicted repeated demand repays transfer/storage cost.
- Transfer immutable model digests over authenticated channels, resume safely, enforce licenses/policies, and verify again on the destination before advertising readiness.
- Never copy proprietary/gated models or prompt-derived adapters to another node without explicit policy authority.

#### Mesh truth and evaluation

- Distinguish agent count, inference request count, routed job count, active-node count, and distributed-model execution; none is inferred from another.
- Claim mesh acceleration only when routing telemetry proves more than one node executed jobs and end-to-end task time improves against the same single-node baseline.
- Benchmark single long requests, parallel subagents, mixed models, warm/cold pools, heterogeneous devices, gaming foreground load, node churn, and constrained Wi-Fi/Ethernet.
- Test split brain, duplicate IDs, pairing attack, DNS rebinding, IPv4/IPv6 movement, revoked nodes, controller restart, asymmetric partition, stale snapshots, lease loss, mixed versions, sleeping laptops, battery thresholds, model mismatch, partial streams, and owner reclaim.

Mesh acceptance criteria:

- a new device can be discovered, mutually authenticated, and contributing an already-present model in under five minutes without copying a long-lived shared secret;
- an unchanged OpenAI-compatible client can use one local URL and receive requests from at least three eligible heterogeneous nodes;
- five simultaneous Knossos subagent calls distribute according to real capacity while a sequential parent session remains warm and stable;
- removing, sleeping, gaming on, or disconnecting one node stops new work there without breaking unrelated requests;
- every route is explainable and every prompt/response remains within the approved local trust set;
- the mesh remains useful as a single-node endpoint and never claims pooled VRAM or sharding for ordinary request routing.

### H14. Observability, privacy, and supportability

- Emit structured logs with stable event IDs for boot, detection, model acquisition, process lifecycle, admission, eviction, request completion, auth decisions, updates, and recovery.
- Publish metrics for request latency/TTFT/tokens per second, queue depth, active streams, context/KV use, endpoint restarts, model load time, VRAM/RAM/disk pressure, GPU utilization, temperature, power, and throttling when available.
- Keep metric labels bounded; never use prompt, user text, arbitrary model path, secret, or untrusted client value as a label.
- Add configurable retention, rotation, disk ceilings, and privacy levels. Prompts and generated content are not logged by default.
- Produce one redacted `cameo doctor --bundle` archive with manifest, versions, hardware facts, health, recent sanitized events, configuration diff from defaults, and an exact preview before export.
- Detect missing/incorrect time synchronization and include monotonic ordering so audits remain useful when wall time jumps.
- Show actionable alerts for degraded storage, corrupt model, repeated crash, thermal throttle, expiring credential, failed backup, unsupported update, and lost mesh node.
- Supply dashboards as a convenience over the same public metrics/events contract; do not create console-only truth.

### H15. Capacity, performance, power, and thermals

- Replace placeholder fit constants and command flags with calibration records from real supported hardware and pinned runtime versions.
- Model weights, KV cache, runtime overhead, graph/scratch memory, host spill, context length, batch size, concurrency, and display-reserved VRAM separately.
- Run a small preflight allocation/load probe before admitting a new model where backend support permits; reserve safety headroom and degrade explicitly.
- Track predicted versus observed memory and latency, then reject or quarantine combinations whose error exceeds the allowed bound.
- Define `quiet`, `balanced`, and `performance` profiles with visible effects on concurrency, clocks/power limits where safely supported, and thermal ceilings. Defaults never overclock hardware.
- Detect sustained throttling, thermal runaway, fan/telemetry loss, and power instability; reduce admission or stop workloads before system damage or repeated crashes.
- Benchmark cold load, warm load, TTFT, decode throughput, memory, wall power where available, and quality sanity across representative context/concurrency points.
- Results include machine fingerprint, BIOS/firmware, kernel, driver/runtime, model digest, backend, settings, ambient caveat, and variance across repeated runs.
- Make “recommended model” a measured fit/quality/latency decision for this machine, not parameter-count marketing.

### H16. Cameo end-to-end acceptance matrix

For every certified hardware combination, stable release qualification must prove:

1. verify artifact signatures and boot/install from release media;
2. create the operator identity and establish the secure local/remote console path;
3. detect and explain the hardware accurately, including unsupported components;
4. run the bundled starter model entirely offline through CLI, console, `/v1`, and Knossos;
5. download/import, verify, start, stream, cancel, stop, restart, evict, delete, and restore a model;
6. survive daemon kill, backend crash, process orphan, power loss, full disk, corrupt partial, GPU reset, and occupied port;
7. enforce authentication, authorization, rate/concurrency limits, secret redaction, and network exposure defaults;
8. preserve models and state through a successful update and automatic failed-update rollback;
9. produce and restore a redacted support/backup bundle;
10. run a 24-hour mixed-load soak with bounded memory, storage, restart count, temperatures, queues, and error rate.

Mesh qualification additionally proves discovery, enrollment, TLS identity, request routing, affinity, owner reclaim, model transfer, node loss, partition recovery, mixed versions, and reservation expiry across the published multi-device matrix. Experimental distributed-model combinations have separate sharding correctness and performance gates. No mocked or fixture-only result can mark a hardware or mesh combination certified.

## 12. Workstream I — CI, supply chain, and releases

### I1. Knossos CI

Required pull-request jobs:

- Rust format, clippy with warnings denied, unit/integration tests, and release build;
- Python format/lint/type/test for the reference implementation;
- ACP and `serve` conformance;
- Field unit, API, browser E2E, accessibility, security, and production build;
- VS Code extension compile, test, and package;
- schema compatibility fixtures;
- npm audit, Rust advisory audit, license policy, secret scan, and SBOM generation;
- Windows, Linux, and macOS matrix for supported Knossos components.

Live-provider and performance suites can run as protected scheduled/release jobs with secrets and explicit cost ceilings.

### I2. Cameo CI

Required pull-request jobs:

- Rust format, clippy, tests, and release build;
- shell lint and unit tests for install/build scripts;
- mock daemon, proxy, auth, node, and `cameo-engine/v1` integration tests;
- container build and smoke test;
- configuration migration tests;
- advisory, license, secret, and SBOM checks.

Protected Linux jobs build the ISO. Hardware jobs run on owned AMD runners and attach machine-readable results.

### I3. Release artifacts

Knossos releases should publish:

- CLI/harness archives for Windows x64, Linux x64/arm64, macOS x64/arm64 as supported;
- Field production bundle or desktop/service package;
- VSIX;
- schemas and conformance fixtures;
- checksums, signatures, SBOM, provenance, release notes, and migration notes.

Cameo releases should publish:

- full and lite ISO images;
- container image by digest;
- standalone daemon/CLI artifacts where supported;
- hardware support snapshot;
- checksums, signatures, SBOM, provenance, release notes, and upgrade/rollback notes.

### I4. Versioning and channels

- Use SemVer for Knossos binaries, Field, extension, and protocols.
- Use a documented distro version plus component manifest for Cameo.
- Channels: nightly, alpha, beta, release candidate, stable.
- Tags are immutable and created only from clean protected branches.
- Every release records compatible Knossos, Field, Cameo, and protocol ranges.
- Keep the current pre-rename `v0.1.0` as historical; the first cohesive product release receives a new version.

### I5. Public repository governance

- Choose and publish an explicit license for Knossos and Cameo; document any component with a different license and verify redistribution rights for models, fonts, artwork, firmware, and bundled binaries.
- Decide DCO versus CLA before accepting outside contributions and automate the selected check.
- Publish maintainer ownership, release authority, review requirements, supported branches, vulnerability response targets, and a succession process.
- Reserve and verify project names across Git hosting, package registries, container registries, domains, and social/documentation surfaces before announcing the split.
- Scan the complete public history and release artifacts for credentials, private URLs, personal data, proprietary code, oversized binaries, and incompatible licenses. Rewrite history only through a separately reviewed migration plan before public release.
- Enable protected branches, required checks, signed/tagged releases, least-privilege CI identities, environment-scoped publishing credentials, and two-person approval for stable publication.
- Add issue/PR templates, code owners, security reporting, support boundaries, a public roadmap, and labels that distinguish defects, proposals, security work, and compatibility changes.
- Record repository-split provenance so Cameo and Knossos retain legally correct attribution and a traceable history without keeping unrelated private material.

Governance exit gate: an external contributor can clone, build, understand the license, report a vulnerability privately, submit a compliant change, and determine who supports the release without contacting the author out of band.

## 13. Documentation and support

Each repository needs:

- concise product README;
- quickstart that succeeds from a clean supported machine;
- architecture and threat model;
- security policy and private reporting route;
- contribution guide and code of conduct;
- changelog and versioning policy;
- support matrix;
- troubleshooting/doctor guide;
- privacy and telemetry statement;
- asset provenance and third-party notices;
- upgrade, rollback, backup, restore, and uninstall instructions.

Create one integration guide titled “Run Knossos on Cameo” and test every command it contains as part of the release checklist.

## 14. Execution sequence

### Milestone 0 — Baseline and repository normalization

Deliverables:

- preserve current work on reviewed branches;
- import Field into `Knossos-Harness/field/`;
- remove or repair the stale editor submodule;
- classify every dirty file as intended product work, generated output, or discard candidate;
- establish CI without changing behavior;
- record current test, bundle, performance, and evaluation baselines;
- create the parity, protocol, and asset inventories.

Exit gate: both repositories build and test from fresh clones, and no release input is untracked.

### Milestone 1 — Security closure

Implement Workstream A and the P0 portions of Workstream B.

Exit gate: the browser-origin RCE, XSS, symlink/junction, secret inheritance, and unenforced-scope test suites all fail against the old implementation and pass against the new one.

### Milestone 2 — Truth and lifecycle closure

Implement Workstreams C and D.

Exit gate: no synthetic, discovered, completed, or role-based fact can become verified without a verdict; assignments and routines converge correctly across completion and restart.

### Milestone 3 — Knossos product consolidation

Implement Workstream E and stable contracts.

Exit gate: Rust passes the product parity matrix, hard suite, editor integration, one cloud engine, one generic local engine, and Cameo mock engine. A first-time user can complete the mission-contract loop from task through independent proof and accept/revise/revert without configuring agents manually. Long missions survive repeated compaction, process restart, provider switching, live steering, and bounded child work without losing contract state, evidence, permissions, or verification truth.

### Milestone 4 — Field polish

Implement Workstreams F and G.

Exit gate: Rome and Atlas pass desktop/compact/mobile product-state review, WCAG audit, 30-agent crowding test, and performance budgets.

### Milestone 5 — Cameo appliance closure

Implement Workstream H through mock and virtualized tests.

Exit gate: the v1 capability contract is consistent across every public surface; daemon, CLI, console, Cameo Link, `/v1` conformance, durable supervisor, model catalogue, simulated mesh discovery/pairing/routing, secret/network boundary, container, ISO build pipeline, installer/storage fixtures, offline bundles, transactional update, backup/restore, and recovery tests pass without undocumented steps.

### Milestone 6 — Hardware beta

Run the owned AMD hardware matrix and the full Knossos-on-Cameo workflow.

Exit gate: at least one validated machine in each advertised hardware class passes boot, hardware-aware recommendation, clean one-action runtime/model setup, model lifecycle, API streaming/cancellation, Knossos task, Field control, crash/power-loss recovery, thermal/capacity validation, upgrade, rollback, and the 24-hour mixed-load soak. A minimum three-device topology additionally proves automatic discovery, secure pairing, heterogeneous request routing, mesh-aware recommendation, session affinity, owner reclaim, node sleep/loss, and single-node degradation. Only evidenced hardware and mesh combinations may be labeled `certified`.

### Milestone 7 — Release candidate

- freeze protocols and user-facing names;
- run all CI, live-provider, browser, security, performance, packaging, and hardware gates;
- perform clean-machine documentation walkthroughs;
- produce signed artifacts, SBOMs, checksums, and provenance;
- resolve every P0/P1 issue or document a narrowly scoped accepted risk;
- run a 7-day dogfood period using only release-candidate artifacts.

Exit gate: no repository checkout, development server, manually copied file, or undocumented credential is needed to use the products.

### Milestone 8 — Stable release and maintenance

- publish Knossos and Cameo compatibility manifests;
- monitor crash/failure reports without collecting source code or prompts by default;
- publish patch policy and supported-version window;
- run monthly dependency/advisory review and quarterly restore drills;
- maintain nightly conformance and hardware smoke runs;
- feed real support failures back into doctor checks and regression fixtures.

## 15. Suggested issue order

Open issues in this dependency order:

1. `KNS-SEC-001` Field authentication and one-time browser bootstrap.
2. `KNS-SEC-002` Origin/Host/CSRF/WebSocket enforcement.
3. `KNS-SEC-003` Markdown sanitizer and security headers.
4. `KNS-SEC-004` Canonical filesystem containment and sensitive-file policy.
5. `KNS-SEC-005` Child environment, terminal bounds, and WebSocket backpressure.
6. `KNS-POL-001` Unified cross-adapter policy object.
7. `KNS-POL-002` Read/write/tool/environment scope enforcement.
8. `KNS-POL-003` Session/routine/campaign budget enforcement.
9. `KNS-POL-004` Concurrency and delegation enforcement.
10. `KNS-DATA-001` Event provenance and production/synthetic separation.
11. `KNS-DATA-002` Evidence-derived maturity and verification.
12. `KNS-DATA-003` Assignment lifecycle and trace filtering.
13. `KNS-RTN-001` Durable routine state and scheduling correctness.
14. `KNS-REP-001` Import Field and normalize repository/release structure.
15. `KNS-LOOP-001` Mission contract schema, UI, amendments, and persistence.
16. `KNS-LOOP-002` Progressive autonomy router and execution modes.
17. `KNS-LOOP-003` Meaningful progress model, grouped approvals, and result object.
18. `KNS-LOOP-004` Accept/revise/revert, continuation, recovery, and mission lineage.
19. `KNS-LOOP-005` Accepted-capability memory and adoption metrics.
20. `KNS-LOOP-006` Versioned `MissionState`, append-only journal, snapshots, and migrations.
21. `KNS-LOOP-007` Context compiler, token accounting, semantic compaction, manifests, and invariant validator.
22. `KNS-LOOP-008` Provider-neutral action/result protocol and artifact rehydration.
23. `KNS-LOOP-009` Plan DAG, evidence-based progress, replanning, and bounded stuck recovery.
24. `KNS-LOOP-010` Capability-aware tool scheduler, safe parallel reads, locks, cancellation, and idempotency.
25. `KNS-LOOP-011` Hash-bound proof obligations, invalidation, flaky-check handling, and final Oracle gate.
26. `KNS-LOOP-012` Typed child contracts, isolated writers, ownership, handoffs, and independent reviewers.
27. `KNS-LOOP-013` Provider capability negotiation, model/effort routing, prompt caching, and complete cost budgets.
28. `KNS-LOOP-014` Crash recovery, deterministic trace replay, long-context tests, and cross-engine conformance.
29. `KNS-ENV-001` Reproducible mission environments, fingerprints, resource limits, caches, and setup export.
30. `KNS-GIT-001` Dirty-tree-safe worktrees, patch/commit/branch handoff, conflict handling, and explicit publication gates.
31. `KNS-EXT-001` Namespaced tool/MCP/skill registry, trust manifests, lazy loading, SDK, and conformance kit.
32. `KNS-ID-001` Opaque secret handles, scoped short-lived credentials, identity display, rotation, and redaction tests.
33. `KNS-TRUST-001` Instruction trust classes, destination-aware egress, injection/SSRF defenses, and adversarial fixtures.
34. `KNS-SCHED-001` Durable multi-mission queue, locks, fairness, restart recovery, and notification policy.
35. `KNS-VIS-001` Isolated browser tooling, visual/accessibility evidence, artifact viewers, and revision binding.
36. `KNS-HAR-001` Rust/Python parity matrix and product-runtime closure.
37. `KNS-EVAL-001` Hard-suite scoring and honesty evaluation.
38. `KNS-UX-001` Naming and onboarding pass.
39. `KNS-UX-002` World crowding, completion, and metric provenance.
40. `KNS-UX-003` Atlas redesign and responsive/mobile status experience.
41. `KNS-A11Y-001` WCAG 2.2 AA closure.
42. `KNS-PERF-001` Asset cleanup, code splitting, caching, and budgets.
43. `KNS-REL-001` CI, release matrix, VSIX/Field artifacts, SBOM, and signing.
44. `ORG-GOV-001` Product licenses, contributor policy, namespace ownership, history audit, maintainers, and protected publishing.
45. `CAM-SCP-001` Freeze v1 scope, support labels, non-goals, and generated capability manifest; reconcile every conflicting public claim.
46. `CAM-SEC-001` Cameo control-plane, local/remote exposure, rate limits, credential scopes, and node trust re-audit.
47. `CAM-API-001` Versioned OpenAI feature matrix, black-box conformance, streaming, cancellation, errors, QoS, and backpressure.
48. `CAM-ENG-001` `cameo-engine/v1` schema, capability negotiation, leases, request identity, and shared Knossos mock suite.
49. `CAM-MDL-001` Content-addressed model catalogue, provenance/license policy, quarantine, integrity, templates, lifecycle, and offline bundles.
50. `CAM-STATE-001` Durable desired/observed supervisor state, atomic transitions, process adoption, migrations, and crash/power recovery.
51. `CAM-STO-001` Destructive-safe installer, encryption option, storage separation, quotas, backup/restore, and granular factory reset.
52. `CAM-UPD-001` Signed component manifests, drain/preflight, transactional update, health commit, offline update, and automatic rollback.
53. `CAM-ISO-001` Reproducible full/lite ISO, QEMU boot, bare installation, persistence, and installed/live posture tests.
54. `CAM-HW-001` Certified/compatible hardware matrix, calibrated fit data, command flags, known-good manifests, workload-aware `RuntimeProfile` recommendation engine, and unsupported-path truth.
55. `CAM-MESH-001` Cameo Link, discovery/pairing, mTLS identity, live capacity advertisements, one local proxy endpoint, model-aware routing, affinity, owner reclaim, model replication, partitions, and mixed-version tests.
56. `CAM-OBS-001` Structured events, bounded metrics, privacy/retention, alerts, and redacted `cameo doctor --bundle`.
57. `CAM-PERF-001` Capacity calibration, prediction error, concurrency, performance profiles, thermal/power safety, and benchmark protocol.
58. `CAM-UX-001` First-boot workload selection, three-profile comparison, transactional one-action runtime/model setup, proof, rollback, endpoint lifecycle, accessibility, and offline console UX.
59. `CAM-KNS-001` Scoped Knossos bundle and end-to-end offline appliance workflow with reboot, eviction, and interrupted-stream recovery.
60. `CAM-REL-001` Signed Cameo artifacts, model/license provenance, SBOM, component manifest, hardware evidence, and support policy.
61. `RC-001` Cross-product release-candidate qualification.

## 16. Definition of polished

Knossos is polished when a developer can throw it a task, understand and dispatch one concise mission contract, follow meaningful progress without supervising tool plumbing, receive independent proof, and safely accept, revise, or revert the result. They can install it, connect a supported engine, understand every consequential action, recover from failure, and trust that “verified” means independently verified.

Cameo is polished when a user can verify and boot or install it on honestly advertised AMD hardware, understand exactly what is supported, run a verified model offline, securely pair additional devices, and give compatible clients one local endpoint that routes work across available capacity without hiding where it ran. It survives ordinary process/power/storage/GPU/network failures, respects device-owner reclaim and privacy, preserves models and state through update or rollback, diagnoses problems without leaking prompts or credentials, and can recover, unpair, or uninstall without surrendering control of data.

The pair is polished when Knossos discovers Cameo through a stable contract, the local/private routing claim is provable, all permissions and budgets remain enforced across the boundary, and the complete workflow is reproducible from signed release artifacts and public documentation.

Until those statements are true, releases should be clearly labeled alpha, beta, or release candidate rather than stable.
