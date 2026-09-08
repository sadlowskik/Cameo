# Cameo + Knossos productization plan

Status: canonical pre-v1 plan

Recompiled: 2026-09-08

Cameo baseline: `311f7cd` (`v0.2.0-beta.3`)

Bundled Knossos baseline: `a20a3b8` (`v0.2.0-beta.1`)

Next package targets: Cameo `v0.2.0-beta.4`; Knossos and Field
`v0.2.0-beta.2`. They remain unpublished until their artifact gates pass.

This is the only roadmap and completion document for Cameo, Knossos, and
Knossos Field. A checked-in module, passing fixture, or previous green report is
implementation evidence; it is not by itself hardware validation, release
qualification, or proof that a user can complete the workflow from an artifact.

## 1. Product line

### Cameo

Cameo is a local-first inference appliance and control plane for compatible AMD
GPU systems. It detects hardware, recommends a bounded model/runtime profile,
manages model files and `llama-server` processes, exposes an authenticated
OpenAI-compatible subset, and can route whole requests among paired nodes.

The primary delivery is a headless Arch-based ISO. The container is a developer
and deployment alternative, not the appliance's identity. Cameo does not promise
that every AMD card works, pool VRAM across machines, or provide a training
framework.

### Knossos

Knossos is an engine-agnostic coding harness. The Rust `knossos` binary is the
product runtime. It plans and executes bounded coding work, keeps exact and
retrieved repository context, applies policy before consequential actions,
records durable mission state, and verifies results independently of the
model's completion claim.

Knossos can use Anthropic, Ollama, OpenAI-compatible providers, or Cameo as an
engine. The Python implementation under `model/knossos/` is retained for
research, trace curation, and comparison; it is not a second supported product
runtime.

### Knossos Field

Field is Knossos's local operator surface for sessions, campaigns, routines,
budgets, evidence, and multi-agent coordination. Field visualizes and controls
Knossos work; it does not replace the harness, certify a result, or form a
security boundary on its own.

### Daedalus

Daedalus is the experimental model architecture that inspired parts of Knossos.
It is research, not the Cameo runtime, not the Knossos CLI, and not required for
either product to ship.

## 2. System boundary

```text
operator / editor
      |
      +--> Knossos Field ---- intent, policy, evidence ----+
      |                                                    |
      +--> Knossos CLI / ACP / serve ----------------------+
                              |
                              +--> cloud or Ollama engine
                              |
                              +--> cameo-engine/v1
                                      |
                                      +--> cameod --> local llama-server --> GPU
                                      |
                                      +--> Cameo Mesh --> paired cameod nodes
```

Ownership is strict:

- Knossos schedules agent work, tools, context, budgets, and proof.
- Cameo schedules compute, models, endpoints, VRAM leases, and whole requests.
- Field projects durable Knossos state and submits operator intent.
- The model proposes; policy authorizes; deterministic evidence decides whether
  a change is complete.

The repositories remain separate. Cameo consumes a pinned Knossos release or
submodule commit through `cameo-engine/v1`; neither repository may reach into
the other's internals as an integration mechanism.

## 3. Words that have release meaning

Every status claim must use one of these levels:

| Level | Meaning |
|---|---|
| Implemented | A production code path exists and is reachable. |
| Locally verified | The relevant deterministic tests passed on the named checkout and host. |
| Platform verified | The artifact passed on each declared operating system/runtime. |
| Hardware verified | A retained record shows the exact artifact, device, driver, runtime, model, workload, and result. |
| Certified | A published support-matrix entry satisfies the defined fault, performance, thermal, and upgrade gates. |
| Release-qualified | Clean tagged artifacts passed every required automated and manual gate without source-checkout help. |

`stable` in a capability manifest means the software contract is intended to be
compatible. It does not mean the appliance, hardware combination, or product is
stable-release qualified.

## 4. Current position

### Cameo: functional beta, not hardware-qualified

Implemented in the current tree:

- fixture-backed GPU discovery, topology, tiering, overrides, and placement;
- model catalogue aliases, SHA-256 verification, GGUF inspection, disk preflight,
  partial cleanup, recommendation, and preview one-action setup;
- CLI plus authenticated `cameod` console/control plane;
- chat completions, completions, embeddings, SSE relay, request limits,
  cancellation/drain handling, and explicit rejection of unsupported features;
- endpoint lifecycle, LRU eviction, session/VRAM leases, checksummed persistent
  desired state, bounded restart attempts, backup/restore, and planned shutdown;
- role-scoped keys, local posture, rate limiting, one-time mesh pairing,
  per-node credentials, deterministic request-level mesh admission, affinity,
  and idempotent reservations;
- universal/lite ISO profiles, offline guided disk installation, starter-model
  seeding, A/B update machinery, build/publish workflows, container recipe, and
  generated capability surfaces.

Evidence observed on 2026-09-08: `cargo test --workspace` passed on Windows.

Not established:

- a retained successful build, boot, install, upgrade, rollback, and recovery run
  for the current ISO on real hardware;
- a non-empty certified AMD support matrix or calibrated memory/performance data;
- Secure Boot qualification, disk-fault/power-loss qualification, encryption,
  quotas/reserve policy, or replacement-machine restore;
- mTLS-bound mesh identity, automatic discovery, remote cancellation/reclaim,
  replication, mixed-version behavior, or a three-node partition test;
- SBOMs, artifact signatures/provenance, full model license records, or a clean
  stable release ceremony.

### Knossos: substantial Rust harness with a green deterministic baseline

Implemented in the current pinned tree:

- Rust CLI, REPL, `serve`, ACP, evaluation, and Field launcher;
- exact symbol indexing, lexical retrieval, bounded context, planning, tool
  dispatch, file staging/diffs, interjection, one-level delegation, and retries;
- workspace jail, restricted argv execution, child-environment scrubbing,
  approval hooks, permission protocol, and offline-by-default tools;
- tiered verification, pre-change baselines, test-deletion detection, proof
  invalidation, explicit halting, and non-verifying dry runs;
- durable mission journal/snapshots, instruction/environment drift checks,
  checkpoint restore, uncertain-action records, accept/revise/revert primitives,
  and provider/Cameo adapters.

Evidence observed on 2026-09-08: all 572 tests passed. The cross-process recovery
gate now verifies the intended durable semantics: follow-up scope increments the
contract revision, preserves the original contract in history, and records the
operator amendment exactly.

Not established:

- complete Linux/macOS/Windows execution and process-tree evidence;
- live cloud, live local, and live Cameo conformance on one release commit;
- durable multi-mission fairness, resource locks, provider-enforced cost ceilings,
  opaque secret handles, DNS/redirect-aware egress, or isolated parallel writers;
- a non-saturated, externally reviewable evaluation demonstrating that Knossos
  improves correctness and honesty rather than merely consuming more steps;
- signed packages, SBOM/provenance, clean version alignment, and an artifact-only
  install/upgrade path.

### Field: broad local implementation, incomplete release evidence

Implemented in the current pinned tree:

- authenticated loopback bootstrap, origin/host/cookie/WebSocket controls, logout
  and revocation, bounded request bodies, sanitized Markdown, path containment,
  terminal policy, environment allowlists, and capability-based permissions;
- versioned event log, replay, production/rehearsal separation, campaigns,
  evidence-derived state, assignments, findings, independent verdicts, budgets,
  routines, pagination, and stress fixtures;
- Knossos session adapter, Cameo-aware displays, Rome/Atlas UI, asset provenance
  manifest, bundle budgets, and release audit tooling.

Evidence observed on 2026-09-08: every test runnable without nested child-process
creation passed, including security, policy, durability, recovery, WebSocket,
simulation, world, and the 100,004-event stress case. The complete suite still
stops at `process-generation.test.mjs`, and Vite/esbuild cannot start, because
this execution sandbox denies Node child spawn with `EPERM`. That is an
environmental block, not evidence that process generation or the production
bundle passes.

Not established:

- a complete supported-platform CI matrix and live provider matrix;
- an end-to-end WCAG 2.2 AA audit and browser performance record;
- hostile-repository, hostile-tool, and deployment-edge security review;
- durable distributed scheduling/fairness beyond the current routine and registry
  implementations;
- a signed standalone Field bundle exercised by a novice from a clean machine.

## 5. v1 scope

### Cameo v1 must

1. Install from a signed, reproducible artifact onto a supported AMD machine.
2. Truthfully identify hardware and offer only evidence-backed profiles.
3. Start a pinned local model and complete chat through the console and `/v1`.
4. Preserve models, configuration, credentials, and endpoint intent across reboot.
5. Update transactionally and automatically return to the prior bootable state
   when health checks fail.
6. Produce a redacted support bundle and a restorable backup.
7. Run bundled Knossos against the local engine without internet.
8. Label every capability and hardware combination by evidence level.

### Knossos v1 must

1. Complete the task -> evidence -> accept/revise/revert loop consistently from
   CLI, REPL, ACP, and `serve`.
2. Survive process restart without losing contract, policy, quota, evidence,
   conversation authority, or uncertain-action state.
3. Fail closed on unverifiable work, changed instructions, unsafe paths,
   unavailable tools, and ambiguous consequential actions.
4. Bound steps, time, context, output, cost, child work, and concurrent work.
5. Produce a portable result object containing changes, verification evidence,
   residual risk, and recovery instructions.
6. Support one cloud engine, Ollama, and Cameo through tested capability
   negotiation rather than provider-specific assumptions.
7. Ship Field and editor integration as thin clients over the same runtime.

### Explicit non-goals for v1

- training a competitive Daedalus model;
- claiming every AMD GPU is supported;
- pooling VRAM or sharding one model across consumer LAN nodes;
- Kubernetes as a supported appliance path;
- unrestricted shell or ambient credentials in Knossos;
- autonomous publishing, deployment, spending, or destructive work without an
  explicit policy and approval boundary;
- using simulated, fixture, or rehearsal state as production evidence.

## 6. Release blockers

### P0: block the next public beta

- Broken download, checksum, install, authentication, or first-chat path.
- A known path that can expose credentials or escape the selected workspace.
- Capability, hardware, or verification copy that contradicts retained evidence.
- Dirty, missing, or manually copied release input.
- Failing deterministic tests in the component being released.
- An update path that can leave both boot slots unusable.

### P1: block v1 release candidate

- Any open item in the v1 scope above.
- Missing current-ISO hardware records for each advertised class.
- Missing cross-product offline and recovery acceptance.
- Missing supported-platform matrix, independent security review, accessibility
  audit, SBOM, signatures/provenance, or version compatibility record.
- Any evaluation in which completion can be obtained by doing nothing, weakening
  tests, omitting work, trusting the model's own claim, or exploiting the grader.

## 7. Workstreams

### A. Release and documentation truth

Owner: cross-product release maintainer.

- Align Cargo/package/display versions and document the Cameo-to-Knossos pin.
- Generate download metadata; never hand-edit versioned artifact URLs in the site.
- Keep one capability manifest and make docs, CLI, console, ISO, and site drift
  checks mandatory.
- Add a documentation link checker and reject references to deleted plans.
- Publish a support policy, compatibility window, release channels, deprecation
  policy, and security-response ownership.

Exit: a clean clone can reproduce every public claim from source and retained
evidence; no roadmap or status document competes with this file.

### B. Knossos correctness and recovery

Owner: Knossos runtime.

- Decide whether operator revisions are part of the durable accepted contract;
  update implementation, schema, and `conversation_recovery` together.
- Run the entire Rust suite after the fix on Windows, Linux, and macOS.
- Complete accept/revise/revert lineage across all four interactive interfaces.
- Reconcile unfinished external actions after restart; never blindly replay them.
- Add schema migration fixtures and repeated-compaction invariant tests.
- Add resource locks and idempotency keys before parallel writes or external calls.

Exit: repeated forced termination at every journal/action boundary resumes to one
explainable state with no duplicate consequential action and current proof only.

### C. Reward-hacking-resistant evaluation

Owner: evaluation maintainer independent of runtime changes.

- Freeze hidden tests and pre-change baselines outside the agent-writable tree.
- Score requested behavior, preserved behavior, scope discipline, honesty,
  efficiency, and recovery separately; never collapse them into one pass bit.
- Make no-op, test deletion, assertion weakening, fabricated verification,
  partial-task omission, and excessive-step solutions explicit adversarial cases.
- Use paired control arms: same model without Knossos, Knossos with context features
  ablated, and at least one stronger/weaker engine pair.
- Separate provider, infrastructure, and behavioral failures. Publish denominators,
  degraded runs, retries, output shrinking, and human overrides.
- Require held-out suites that are not saturated before making a capability claim.

Exit: an external reviewer can reproduce the score and identify whether a gain
came from the harness, the model, more compute, data leakage, or grader gaming.

### D. Cameo appliance and hardware qualification

Owner: Cameo appliance.

- Build universal and lite ISOs twice from a clean tag and compare declared
  reproducible outputs.
- Boot in QEMU; install to virtual disk; reboot; validate persistence, A/B update,
  interrupted update, fallback, backup, restore, and factory-reset boundaries.
- Run `scripts/phase1/RUNBOOK.md` on representative legacy Vulkan, current consumer
  ROCm, datacenter ROCm, and APU/host-offload systems.
- Record exact artifact digest, firmware, kernel, Mesa/ROCm, GPU, VRAM/RAM, model
  digest, parameters, throughput, memory error, temperature, power, and failures.
- Calibrate recommendation margins from those records and publish only observed
  combinations as certified.
- Add stable disk identity/recheck, storage reserve and quotas, optional encryption,
  and safe recovery after interruption.

Exit: every advertised class completes install, first chat, sustained load,
restart, update, rollback, and support-bundle capture using only release artifacts.

### E. Cameo serving, models, and durable state

Owner: Cameo control plane.

- Black-box the published OpenAI subset against a pinned runtime: sync/streaming,
  cancellation, malformed framing, disconnect, overload, and fair concurrency.
- Finish safe process adoption and reconcile OOM, driver reset, occupied port,
  missing model, corrupt state, and clock changes without phantom readiness.
- Store immutable model provenance, license, source, digest, GGUF metadata,
  compatible templates, and import/export lineage; quarantine unknown input.
- Make update-state schema compatibility and rollback behavior explicit.
- Add structured event IDs, retention/rotation, alert thresholds, and privacy levels.

Exit: crash and fault injection cannot produce an unowned process, false-ready
endpoint, lost capacity, silent model substitution, or unrecoverable state.

### F. Cameo Mesh

Owner: Cameo networking.

- Add automatic local discovery without treating discovery as trust.
- Bind paired device identity to mTLS certificates with rotation and revocation.
- Advertise fresh capacity with bounded leases and reclaim ownership after loss.
- Add remote cancellation, session affinity, overload behavior, model replication
  policy, and mixed-version compatibility.
- Test three heterogeneous nodes under partition, sleep, flapping, stale metrics,
  hub restart, node restart, and single-node degradation.

Exit: one stable endpoint routes whole requests safely and predictably; the UI
never implies pooled VRAM or distributed execution.

### G. Field product closure

Owner: Field.

- Complete the process-generation test and production build outside this sandbox.
- Qualify child-process termination and secret isolation on all supported systems.
- Finish durable admission/fairness for concurrent missions and routine misfires.
- Exercise campaigns with real Knossos engines, not only fixtures.
- Run keyboard, screen-reader, zoom, contrast, reduced-motion, responsive, LCP,
  CLS, interaction, and 30-agent sustained-stream tests.
- Package an audited production bundle with immutable assets and clear first-run,
  empty, degraded, permission, failure, and recovery states.

Exit: a new user can install Field, start a bounded mission, understand every
permission and failure, recover it after restart, and export its evidence.

### H. Cross-product offline acceptance

Owner: release maintainer; witnessed by an independent tester.

- Install Cameo from the candidate ISO on a supported AMD machine.
- Start the bundled model and verify `/v1` sync and SSE responses.
- Launch the bundled Knossos artifact with `--engine cameo` and no internet.
- Complete a repository change through Field or ACP, inspect the result, accept it,
  reboot, and confirm mission and endpoint recovery.
- Repeat with a refused oversized load, leased-model eviction conflict, cancelled
  stream, daemon restart, Knossos restart, and failed update rollback.

Exit: the artifact-only transcript contains hashes and evidence for every step and
requires no repository checkout, development server, or undocumented credential.

### I. Supply chain, governance, and support

Owner: release maintainer.

- Protect release branches/tags and require CI, review, and clean submodules.
- Generate checksums, SBOMs, signatures, provenance attestations, component/model
  licenses, compatibility manifest, and rollback instructions.
- Establish maintainer and namespace ownership, contributor policy, security SLA,
  telemetry/privacy policy, and supported-version window.
- Keep signing and provider secrets out of child processes and release logs.

Exit: a release can be independently verified, serviced, revoked, and reproduced.

## 8. Execution order

1. **Restore a green baseline.** Fix Knossos recovery semantics; rerun Cameo,
   Knossos, Field, docs generation, and link checks from clean trees.
2. **Normalize releases.** Align versions, pin boundaries, artifact manifests,
   downloads, CI matrices, and clean packaging.
3. **Prove each product alone.** Complete Knossos recovery/evaluation, Field
   platform/browser qualification, and Cameo ISO/hardware qualification.
4. **Close security and durability.** Independent review plus fault matrices for
   secrets, workspaces, processes, storage, updates, and mesh identity.
5. **Run the cross-product path.** Artifact-only offline Cameo + Knossos + Field
   acceptance on real AMD hardware.
6. **Cut an RC.** Freeze scope and compatibility; run a 24-hour mixed workload,
   version-to-version upgrades, rollback, three-node mesh, and seven days of
   artifact-only dogfood.
7. **Release v1.** Publish only after every P1 item has retained evidence and no
   exception is hidden in prose.

## 9. Immediate issue queue

Work in this order; each item must leave the relevant suite greener or more
truthful than it found it.

1. `KNS-COR-001` — complete: durable-contract recovery semantics are explicit and
   the full Knossos suite passes.
2. `DOC-001` — complete: local links, retired-document references, capabilities,
   and generated release metadata are CI gates.
3. `REL-001` — in progress: Cameo, Knossos, Field, Cargo/npm, and display versions
   are aligned; current Cameo ISO artifacts still need to be built and published.
4. `REL-002` — complete: `release-manifest.json` generates the site download block,
   and unpublished/mislabeled artifacts cannot appear as working downloads.
5. `FIELD-CI-001`: run the complete Field suite/build where child spawn is allowed.
6. `KNS-EVAL-001` - in progress: the eight-case reward-hacking suite, hardened
   grader, frozen 27-case lock, and equal-budget Windows paired-control runner are
   implemented; run the live engine matrix, retain traces, obtain independent
   review, and publish the evidence.
7. `CAM-ISO-001`: retain current-tag universal/lite build and QEMU install evidence.
8. `CAM-HW-001`: collect the first complete real-AMD qualification record.
9. `INT-001`: automate the mock Cameo/Knossos contract suite in both repositories.
10. `RC-001`: script the artifact-only offline acceptance and fault matrix.

## 10. Required release evidence

Every release candidate stores, by immutable artifact digest:

- source/tag/submodule revisions and dependency lockfiles;
- CI results for supported platforms and generated-document drift;
- SBOM, licenses, checksums, signatures, and provenance;
- Cameo ISO/container build, boot/install/update/rollback results;
- hardware matrix records and performance/thermal measurements;
- OpenAI and `cameo-engine/v1` conformance results;
- Knossos deterministic, recovery, adversarial, live-engine, and ablation results;
- Field security, process, browser, accessibility, performance, and package results;
- cross-product offline transcript, fault matrix, soak, and dogfood report;
- known limitations, compatibility window, support owner, and rollback procedure.

Absence is reported as absence. Skips, degraded provider runs, sandbox restrictions,
and manual interventions remain visible in the retained result.

## 11. Definition of v1 complete

Cameo v1 is complete when a non-developer can turn a listed AMD system into a
recoverable local inference appliance from a verifiable artifact and every support
claim maps to a retained hardware record.

Knossos v1 is complete when a non-developer can install one Rust artifact, run a
bounded coding mission through any supported interface/engine, survive interruption,
and receive independently verified changes without the model or harness being able
to game completion.

The combined product is complete when that Knossos workflow runs offline on a
Cameo appliance, survives the defined compute and process failures, and can be
operated through Field without weakening either product's policy or evidence model.

Anything less remains beta, regardless of test count or visual polish.
