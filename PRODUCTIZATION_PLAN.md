# Cameo + Knossos productization plan

Status: canonical pre-v1 plan

Recompiled: 2026-09-08

Amended: 2026-09-20 (full functionality/size/security audit; hive scope: training
in v1 as an on-demand devkit, Knossos as an on-demand pinned install, single
Vulkan ISO with on-demand ROCm, image-based A/B updates, Mesh discovery).

Amended: 2026-09-20 (Knossos harness assessment; all-Rust runtime including
Field's core; plug-and-play agents over ACP; Field reachable from anywhere
through the Cameo origin and the operator's private network; Knossos CI truth).

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
that every AMD card works or pool VRAM across machines.

The product target is a plug-and-play node: flash, install, and the box serves.
A second box pairs into Cameo Mesh with one code and every node's console shows
the whole pool. Training is in scope on Tier 1 and 2 hardware through an
on-demand devkit (`cameo-devkit`); neither the devkit nor ROCm HIP nor Knossos is
baked into the ISO image. Vulkan is the universal baseline on the image and every
heavier stack is an opt-in install pinned to the image's package snapshot.

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

The harness is all Rust: every process a user runs is the `knossos` binary.
That includes Field's core (event log, projection, adapters, budgets, routines,
director, API and WebSocket) which today runs as a Node sidecar and is ported
under workstream G. The browser client stays a web app because it runs in a
browser; the desktop app is a Tauri shell that links the `knossos` crate
directly (one process, no sidecar) and is usable with any provider, any
project and any ACP agent, with Atlas as its default face. Agents plug in through
the Agent Client Protocol: Knossos is itself an ACP agent, and any ACP agent
(Claude Code, Codex, Gemini CLI, a user's own) becomes a unit on the same map
under one Field-level permission gate. Field is served through the Cameo box's
own HTTPS origin and reached from anywhere over the operator's private network;
there is no public relay and no token reselling.

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

The repositories remain separate. Cameo installs a pinned Knossos release on
demand (`cameo knossos install`, digest-verified) and talks to it only through
`cameo-engine/v1`; neither repository may reach into the other's internals as an
integration mechanism. The `daedalus` submodule is no longer an ISO build input.

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
- optional ROCm CLI examination for read-only GPU/runtime discovery, correlated
  by PCI address with strict runtime-status gating and legacy-probe fallback;
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
  for the current ISO on real hardware. The 2026-09-20 audit found the current
  ISO fails the guided-install preflight (`e2fsprogs` absent), would install a
  root that cannot boot (inherited `mkinitcpio.conf.d/archiso.conf`), gives the
  daemon no GPU device access (`cameo` user lacks `render`/`video`), cannot find
  ROCm tools from units (`/opt/rocm/bin` off PATH), loses the console key on any
  daemon restart (`RuntimeDirectory=`), has an ordering cycle on
  `cameo-firstboot`, and serves `/v1` without a key on any loopback bind. All are
  open (`CAM-ISO-002`, `CAM-SEC-001`);
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
- harness-tool path jail, restricted argv execution, child-environment
  scrubbing, approval hooks, permission protocol, and offline-by-default tools
  (child processes are not yet confined to the workspace; see `KNS-JAIL-001`);
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

- the 2026-09-20 harness assessment found: a mission can halt as Done with exit
  0 on a reused tier-0 quick verify without full verification ever running;
  a crash between a consequential action and its recorded result makes the
  mission permanently unresumable (no reconciliation); resume drops the
  pre-change baseline; accept/revert exist only in `serve`; no mission
  wall-clock or monetary ceiling; the result object lacks residual risk and
  recovery instructions; `Retry-After` is ignored; the sandbox is an
  environment jail while the docs say workspace jail; Field's terminal spawns
  a real shell outside policy and Field launches missions without persistence.
  All open (`KNS-VERIFY-001` … `KNS-RESULT-001`);
- Knossos CI is red for reasons unrelated to the runtime: the Python job never
  installs torch that ten test files import; the ACP conformance job never
  builds the Rust binary it spawns; the "Lapce" job tests an editor that is now
  a VS Code extension; macOS `cargo test` and Field-on-Windows fail without
  readable logs (`KNS-CI-001`);
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

Evidence observed on Windows on 2026-09-09: the complete Field suite passed when
child-process creation was allowed, including process generation, security,
policy, durability, recovery, WebSocket, simulation, world, and the 100,004-event
stress case. The Vite production build also passed. This is local Windows
evidence; rerun the same suite and build in Linux CI with child spawn allowed.

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
7. Install a pinned Knossos release on demand and run it against the local
   engine; the install needs network, the run does not.
8. Label every capability and hardware combination by evidence level.
9. Install the training devkit on demand on a Tier 1 or 2 node and complete a
   bounded single-node fine-tune through `cameo train`.
10. Pair a second node into Cameo Mesh with one code, after LAN discovery
    surfaces it as a candidate, and route a whole request to it.

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
8. Run Field's core inside the `knossos` binary with no Node runtime on the
   box; the Node server is retired once the ported test suite is green.
9. Accept any ACP agent as a unit through the generic adapter, gated by the
   same Field-level permission gate as Knossos's own units, with a conformance
   suite that user-authored adapters must pass.
10. Serve Field through the Cameo box's HTTPS origin behind the console key,
    and reach it from a phone or laptop over the operator's private network
    with sessions resuming on any device.

### Explicit non-goals for v1

- training a competitive Daedalus model;
- claiming every AMD GPU is supported;
- pooling VRAM or sharding one model across consumer LAN nodes;
- multi-node training (rendezvous across nodes) — post-v1;
- baking ROCm HIP, PyTorch, or Knossos into the ISO image;
- Kubernetes as a supported appliance path;
- unrestricted shell or ambient credentials in Knossos;
- a public relay, hosted accounts, or token reselling for "anywhere" access;
- multi-tenant Field (one operator per Field instance in v1);
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
- Close the quick-verify reuse: a reused tier-0 verdict may never satisfy the
  halt condition; `Halt::Done` and a zero exit require a full-mode verdict at
  the current workspace revision (`KNS-VERIFY-001`).
- Reconcile unfinished consequential actions on resume: re-observe the target
  (file digest, command exit, external call receipt) and record an explicit
  outcome instead of refusing resume (`KNS-REC-001`).
- Serialize the Oracle baseline into the journal so a resumed mission judges
  the same pre-existing failures as a fresh one (`KNS-BASE-001`).
- Expose accept, revise and revert on CLI, REPL and ACP, not only `serve`;
  make revise an explicit verb that increments the contract revision
  (`KNS-LIN-001`).
- Project residual risk and recovery instructions into the result object on
  every interface (`KNS-RESULT-001`).
- Add a mission wall-clock deadline and a monetary ceiling to the bounds;
  honour `Retry-After` on 429 (`KNS-BOUNDS-001`).
- State the sandbox honestly: an environment jail with a harness-tool path
  jail; either document that or add OS-level confinement, never claim both.
  Documented as of 2026-09-20; the confinement itself is `KNS-JAIL-001` below.
- Confine child processes to the workspace (`KNS-JAIL-001`, Linux and macOS;
  Windows has no unprivileged filesystem confinement and stays environment-jail
  plus Job Object). The path jail binds the harness's own tools; `cargo test`,
  `pytest`, delegated units, Field's operator terminal and any plug-and-play
  agent Field launches run agent-authored code with the operator's full
  filesystem and network. That is the gap a hostile or careless unit exploits,
  and it grows with every third-party agent, so the gate has to be structural.
  - **One spawn path.** `Sandbox::command` already constructs every child
    (`run` tool, Oracle ladder, delegation). Confinement is applied there,
    between fork and exec, so every descendant inherits it and none can shed
    it. Field's Node server and its terminal spawn through `knossos exec --
    <cmd>` (same `Sandbox`) until the Rust port replaces them, so the port
    inherits the jail rather than retrofitting it.
  - **Linux: Landlock** (`landlock` crate 0.4; kernel >= 5.13, unprivileged, no
    daemon, enabled in the Arch kernel Cameo ships and in Ubuntu runners since
    22.04). Ruleset: read + execute on the system roots (`/usr`, `/bin`,
    `/lib`, `/lib64`, `/etc`, `/opt`, `/proc`, `/sys`, `/run`, `/var`) and the
    toolchain homes (`RUSTUP_HOME`, `$HOME/.local`, `$HOME/.cache`, the Python
    prefix, `$HOME/.npm`); read + write on `CARGO_HOME` (cargo takes a lock in
    its package cache), `/dev`, and a per-run `TMPDIR`; full access including
    execute on the workspace; nothing else. The rest of `$HOME` (SSH keys,
    shell history, sibling repositories) does not exist for the child. Landlock
    resolves the real inode, so a symlink inside the workspace that points
    outside is refused without the manual check the path jail needs. ABI 4+
    can deny TCP bind and connect; this is opt-in (`Sandbox::deny_network`,
    `knossos exec --deny-network`) rather than the default, because a
    project's own test suite is entitled to bind loopback and Landlock's
    network rules are port-based, so loopback cannot be carved out. UDP and
    Unix sockets are not covered. A denial the kernel cannot honour is a
    residual-risk line. Applied in a `pre_exec` hook, the ABI queried from
    the kernel and reported.
  - **macOS: Seatbelt.** Wrap the command in `/usr/bin/sandbox-exec -f
    <profile> -D WORKSPACE=<root> -D TMP=<tmpdir> -- <program> <args>`.
    `sandbox-exec` execs in place, so pid, process group and kill-tree handling
    are unchanged. The profile is `(deny default)` with `(allow process-exec
    process-fork sysctl-read mach-lookup)`, read on the system and toolchain
    paths above, read-write on the workspace, `TMP`, `CARGO_HOME` and `/dev`,
    and `(deny network*)` unless networked. Deprecated interface, still shipped
    on current macOS and relied on by Chromium and Bazel; if Apple removes it
    the run degrades to `EnvOnly` and says so rather than failing silently.
  - **Policy knob** `KNOSSOS_CONFINE=require|prefer|off` (default `prefer`;
    a `--confine` flag can follow once the config surface is consolidated). `require` refuses to spawn when the
    OS cannot confine, and is what CI and the Cameo appliance use; `prefer` runs
    and records the level; `off` is an operator debugging aid and is journaled.
    `Sandbox::allow_path` and `KNOSSOS_CONFINE_ALLOW_RO`/`_RW` admit an extra
    toolchain directory for stacks this list did not anticipate; a denied path
    fails the child loudly with `EACCES` in its stderr, which is the correct
    failure.
  - **Reporting.** `Finished.confinement` is `Landlock { abi }`, `Seatbelt`, or
    `EnvOnly` on every command record and in the journal. A mission in which
    any child ran `EnvOnly` carries "child processes ran without filesystem
    confinement on <platform>" in `Outcome.residual_risk`; on Linux with
    Landlock below ABI 4 it carries "network not confined". The README Safety
    section then reads: workspace jail and environment jail on Linux and
    macOS, environment jail only on Windows.
  - **Tests** (Linux and macOS jobs, skipped on Windows with a notice): a probe
    child reads `/etc/hostname`; its writes to `$HOME` and to a sibling of the
    workspace are refused; a write inside the workspace succeeds; a workspace
    symlink to `$HOME` is refused; `cargo test` of a fixture crate passes under
    the jail (proves the toolchain path list is complete); a TCP connect to
    `127.0.0.1` is refused when offline and allowed when networked;
    `--confine=require` on a kernel without Landlock returns a typed error.
  - **Order.** Spike first: a 40-line probe in CI on `ubuntu-latest` and
    `macos-latest` proving the ruleset and the profile before the module is
    written. Then Linux, macOS, reporting, `knossos exec`, docs. Lands before
    `KNS-RUST-001` so the Field port spawns through a jailed `Sandbox` from its
    first commit.
  - **Cameo.** Phase 0 hardware checklist gains `grep landlock
    /sys/kernel/security/lsm`; the appliance runs Knossos with
    `KNOSSOS_CONFINE=require`, which is a concrete reason to run agents on the
    box rather than on a laptop.

Exit: repeated forced termination at every journal/action boundary resumes to one
explainable state with no duplicate consequential action and current proof only;
no interface can report success without a full-mode verdict.

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

- Ship one Vulkan ISO as the lead artifact; the universal (ROCm-baked) ISO stays
  buildable on manual dispatch for air-gapped Tier 1/2 boxes only. Build twice
  from a clean tag and compare declared reproducible outputs.
- Make the QEMU job a required gate: boot the ISO, assert autologin over serial,
  probe `/readyz`, run the guided installer unattended against a virtual disk,
  reboot from that disk, probe `/readyz` again. Then validate persistence, A/B
  update, interrupted update, fallback, backup, restore, and factory-reset.
- Declare the package closure: `archiso/required-binaries.txt` lists every
  binary any script or unit calls; CI asserts each exists in the built airootfs.
- Take releng files by allowlist, never the whole tree; CI diffs the allowlisted
  files against upstream archiso and fails on drift.
- `cameo rocm install`: Tier 1/2 only, persistent root only, installs `ggml-hip`
  pinned to `/etc/cameo/snapshot`, restarts `cameod`; suggested by first boot
  and the console, never automatic.
- Trim the image: drop `linux-headers`; replace the `linux-firmware` metapackage
  with the AMD, Wi-Fi and `whence` split packages the image can use.
- Console posture: bind the LAN with the generated key by default and print it
  at login; built-in TLS (self-signed at first boot, fingerprint printed) lands
  under workstream E. The SSH-tunnel guidance is removed.
- Run `scripts/phase1/RUNBOOK.md` on representative legacy Vulkan, current consumer
  ROCm, datacenter ROCm, and APU/host-offload systems.
- Record exact artifact digest, firmware, kernel, Mesa/ROCm, GPU, VRAM/RAM, model
  digest, parameters, throughput, memory error, temperature, power, and failures.
- Implement a local-first, moderated community-report and release-credit flow;
  community reports remain distinct from witnessed hardware verification and
  certification.
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
- Move to image-based A/B updates: an update bundle is a signed `airootfs.sfs`;
  apply is the installer's own `unsquashfs` into the inactive slot followed by the
  existing boot trial and health commit. Kernel, Mesa, llama.cpp and ROCm move as
  one validated set; the rsync-and-overlay path is removed.
- Move boot-time logic into the binary (`cameo hello`, `cameo storage`,
  `cameo seed`); the shell scripts become one-line wrappers.
- `/v1` requires a consumer key regardless of bind address; add an absolute
  per-connection deadline and a per-IP connection cap; validate `Host` and
  `Origin`; close the pairing enrollment oracle; escape `\r` in metrics.
- Built-in TLS on `cameod`; managed serving as supervised, restart-on-failure
  services so a rebooted node brings its models back unattended.
- Demote live mode to try-it only: remove `cameo-persist-cache` and the
  `CAMEO_DATA` auto-mount. Install is the one persistence story.
- Add structured event IDs, retention/rotation, alert thresholds, and privacy levels.

Exit: crash and fault injection cannot produce an unowned process, false-ready
endpoint, lost capacity, silent model substitution, or unrecoverable state.

### F. Cameo Mesh

Owner: Cameo networking.

- Add automatic local discovery without treating discovery as trust: a small
  signed UDP multicast beacon from `cameod` (node id, version, gateway address)
  surfaces candidates in the console with a join button; pairing remains the
  only trust step. No Avahi dependency.
- Route pool-wide: the hub's `/v1` forwards by model to the node holding it
  through the existing dependency-free proxy; each node's own `/v1` keeps working.
- Replicate models hub-to-node over the paired channel with digest verification;
  residency and eviction become pool-aware (the hub asks a node to load or evict).
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

Field's component roadmap (`field/docs/plug-and-play-roadmap.md` in the Knossos
repository) holds the phase detail; this workstream fixes the order and the
non-negotiables.

- **Port field-core to Rust as the spine, not as "later".** The Node server's
  responsibilities (event log with SQLite and JSONL backends, projection fold,
  WebSocket coalescing, API with bootstrap-cookie auth and origin/host gates,
  harness adapters, budget ledger and admission, routines, campaign director,
  world projection) move into `knossos field` behind stable interfaces.
  Strangler order: event log and projection first (the suite's replay and
  stress tests are the parity oracle), then API and WebSocket, then adapters
  (Knossos's own adapter becomes in-process), then director and routines. The
  Node server stays selectable by flag until every ported test is green, then
  is deleted with its `npm` dependency. The SPA and `field-event-v1` do not
  change, so the web client needs no rewrite (`KNS-RUST-001`).
- **Plug-and-play agents standardise on ACP.** The generic-ACP adapter is the
  first new adapter; manifest-only CLI and HTTP agents follow. Every adapter,
  including user-authored ones, passes the adapter conformance suite, and one
  Field-level permission gate decides every consequential action regardless
  of what the adapter self-reports (`FIELD-ACP-001`).
- **Field's own terminal and shell go through Knossos policy** or are removed;
  Field launches every mission with persistence on (`FIELD-POLICY-001`).
- **Reachable from anywhere, through Cameo.** `cameod` reverse-proxies
  `/field/` to `knossos field` on loopback, so Field shares the console's
  TLS certificate, key and origin; the console links to it. Off-LAN access is
  the operator's private network: Cameo ships `wireguard-tools` and
  `cameo remote` mints a peer configuration and a QR code for a phone;
  Headscale/Tailscale remain optional. Field ships a PWA manifest so it
  installs on a phone, and the event log is the session store, so any device
  resumes the same operation. Installed by `cameo knossos install`, run as a
  systemd unit under the operator account (`FIELD-REMOTE-001`).
- **Retire Python from the product path.** Daedalus training and trace tooling
  stay in `model/` under a research marker outside the CI gate; the Rust
  evaluator and the frozen suite lock become the only grading truth
  (`KNS-PY-001`).
- Complete the process-generation test and production build outside this sandbox.
- **Desktop app for anyone, on the Rust core** (`KNS-APP-001`). The app is
  the reason for the port, not something after it. Order: (1) event log and
  session API in Rust; (2) the `knossos` binary serves the Field web UI over
  local HTTPS, so the app already works in a browser on every OS with no
  Node anywhere, and it is the same thing `cameod` proxies; (3) Tauri 2 shell
  linking the `knossos` library crate directly: native window and tray,
  provider keys in the OS keychain read only at spawn time, file pickers,
  notifications; (4) first run: pick a model (a Cameo box found on the LAN,
  Ollama on this machine, or a cloud key into the keychain), open a folder,
  confirm the per-project verification contract (cargo, npm test, pytest, go
  test, make; editable), go; (5) ACP agents in the same jail; (6) signed
  installers for Windows, macOS and Linux, a release lock, in-app updates.
  Atlas is the default mode and uses plain words (Models, Projects, Agents,
  Approvals, "needs you"); Rome and the RTS map are the operator mode behind
  a switch, over the same event log. Stated up front in the app: the
  workspace jail exists on Linux and macOS only; on Windows children run
  unconfined and every result says so, and the confined path is WSL2, which
  the app can drive. The app collects nothing and phones nowhere unless a
  Cameo hub is paired.
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
- Install the pinned Knossos release with `cameo knossos install`, then launch it
  with `--engine cameo`; the run itself uses no internet.
- Complete a repository change through Field or ACP, inspect the result, accept it,
  reboot, and confirm mission and endpoint recovery.
- Repeat with a refused oversized load, leased-model eviction conflict, cancelled
  stream, daemon restart, Knossos restart, and failed update rollback.

Exit: the artifact-only transcript contains hashes and evidence for every step and
requires no repository checkout, development server, or undocumented credential.

### J. Training devkit (v1, on demand)

Owner: Cameo appliance.

- `cameo-devkit install`: a Python venv at `/var/lib/cameo/devkit` holding the
  official PyTorch ROCm wheel from a committed hash-locked requirements file;
  refused on the RAM overlay; free-space and Tier 1/2 preflight; `--dry-run`.
- `cameo train` finds `torchrun` on PATH, then in the devkit, and runs as a
  supervised job with a VRAM reservation the placement engine respects;
  training and inference share one admission path and refuse rather than
  oversubscribe.
- Multi-node `torchrun` with the hub as rendezvous is post-v1.

Exit: a bounded fine-tune completes on one Tier 1 node from release artifacts,
with the devkit install, the run, and the reservation visible in the console.

### K. Knossos delivery

Owner: cross-product release maintainer.

- `cameo knossos install <version>`: downloads the pinned Knossos release,
  verifies its digest against a committed lock, installs to persistent storage.
- Remove the `daedalus` submodule from the ISO build and the `knossos` binary
  from the image; the `cameo-engine/v1` contract and its mock conformance suite
  remain the only coupling.

Exit: a fresh install can obtain Knossos with one command and pass the
`cameo-engine/v1` conformance run against the local engine.

### L. Knossos CI truth

Owner: Knossos runtime.

- Build the Rust binary before the ACP conformance job spawns it.
- Guard or install torch for the Python job; move Daedalus tests behind the
  research marker.
- Publish failing job logs to a public `ci-logs` branch as Cameo does, so
  macOS and Windows failures are diagnosable without credentials.
- Retire or rename the Lapce job to match the VS Code extension that exists.
- Make the Knossos submodule/pin in Cameo track a green mainline commit.

Exit: every job on the Knossos default branch is green or explicitly skipped
with a printed reason, and a red job has a readable log.

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

1. **Restore a green baseline.** Fix the 2026-09-20 audit P0s (`CAM-ISO-002`,
   `CAM-SEC-001`) and land the required QEMU boot-install-reboot gate, the
   required-binaries manifest and the releng allowlist; fix Knossos recovery
   semantics; rerun Cameo, Knossos, Field, docs generation, and link checks from
   clean trees.
2. **Normalize releases.** Align versions, pin boundaries, artifact manifests,
   downloads, CI matrices, and clean packaging.
3. **Prove each product alone.** Complete Knossos recovery/evaluation, Field
   platform/browser qualification, and Cameo ISO/hardware qualification.
   Knossos order within this step: L (CI truth) → B's `KNS-VERIFY-001` and
   the restart pair (`KNS-REC-001`, `KNS-BASE-001`) → lineage and result
   object → `KNS-JAIL-001` (Landlock and Seatbelt child confinement) →
   G's Rust port (event log first) with `KNS-PY-001` alongside →
   ACP adapter and Barracks/Power-sources/Folders UI → Cameo origin and
   private-network access → RTS surface, which can start in parallel on the
   web side because it is a projection over an unchanged event schema →
   the desktop app (`KNS-APP-001`) as soon as the binary serves the web UI,
   then the shell, first run, agents and installers.
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
5. `FIELD-CI-001` — complete locally: the full Field suite and production build
   pass on Windows with child spawn allowed; retain the same run in Linux CI.
6. `CAM-ROCMCLI-001` - in progress: the optional read-only ROCm CLI examination
   adapter is implemented and locally tested. Qualify it on native Windows,
   Linux, and WSL AMD systems; pin a supported upstream contract/version before
   promoting it from preview. Do not delegate serving, updates, process recovery,
   or multi-GPU placement until ROCm CLI exposes stable machine-readable
   lifecycle contracts and those paths pass Cameo's fault matrix.
7. `CAM-REPORT-001` - in progress: the versioned contracts, privacy-allowlisted
   local report, exact payload preview/hash, consent gates, failure path, and
   bounded observed starter-model smoke are implemented; validate the smoke on a
   real Cameo image.
8. `CAM-INTAKE-001` - complete: the strict, size-bound, salted-rate-limited
   Pages/D1 pending intake is deployed at the production domain, and authenticated
   local export/approve/reject tooling keeps moderation off the public API.
9. `CAM-MATRIX-001` - in progress: one reviewed approved-report source generates
   the consented roster, offline credits, and aggregate public hardware matrix;
   approve the first real reports before making compatibility claims.
10. `CAM-CREDITS-001` - in progress: `cameo credits` and ISO embedding are wired;
    set the release cutoff, review consent records, and freeze the release asset.
11. `KNS-EVAL-001` - in progress: the eight-case reward-hacking suite, hardened
   grader, frozen 27-case lock, and equal-budget Windows paired-control runner are
   implemented; run the live engine matrix, retain traces, obtain independent
   review, and publish the evidence.
12. `CAM-ISO-002` (P0): fix the audit boot/install blockers — `e2fsprogs`,
    delete inherited `mkinitcpio.conf.d/archiso.conf` and
    `sshd_config.d/10-archiso.conf`, `render`/`video` for the `cameo` user,
    `/opt/rocm/bin` on unit PATH and in `gpu-detect`, `cameo-firstboot`
    ordering, `RuntimeDirectory=` key loss, `hostname` → `ip`, root shell bash,
    installer slot sizing; then make the QEMU install gate required.
13. `CAM-SEC-001` (P0): `/v1` consumer key always; connection deadline and
    per-IP cap; `Host`/`Origin` checks; pairing oracle; metrics escaping.
14. `CAM-ISO-003`: single Vulkan lead ISO, universal on manual dispatch,
    `cameo rocm install`, firmware/headers trims, LAN-bind console default.
15. `CAM-UPD-002`: image-based A/B update bundle and apply path.
16. `CAM-DEVKIT-001`: training devkit and `cameo train` integration (workstream J).
17. `CAM-KNS-001`: `cameo knossos install`; remove the bundled binary and the
    submodule build input (workstream K).
18. `CAM-MESH-002`: discovery beacon, pool-wide `/v1` routing, replication.
19. `CAM-ISO-001`: retain current-tag build and QEMU install evidence.
20. `CAM-HW-001`: collect the first complete real-AMD qualification record.
21. `INT-001`: automate the mock Cameo/Knossos contract suite in both repositories.
22. `RC-001`: script the artifact-only acceptance and fault matrix.
23. `KNS-CI-001` (P0): conformance job builds the binary; torch guard or
    install; ci-logs publishing; retire the Lapce job (workstream L).
24. `KNS-VERIFY-001` (P0): no Done/exit-0 on a reused tier-0 verify.
25. `KNS-REC-001`, `KNS-BASE-001`: reconcile pending actions and keep the
    baseline across resume.
26. `KNS-LIN-001`, `KNS-RESULT-001`, `KNS-BOUNDS-001`: accept/revise/revert on
    every interface; residual risk and recovery in the result; time and money
    ceilings; `Retry-After`.
27. `KNS-RUST-001`: field-core in Rust, strangler order, Node server deleted
    at parity.
28. `FIELD-ACP-001`, `FIELD-POLICY-001`: generic ACP adapter and conformance
    suite; Field terminal under policy; persistence on by default.
29. `FIELD-REMOTE-001`: cameod `/field/` proxy, `cameo remote` WireGuard
    helper, PWA manifest, systemd unit under the operator account.
30. `KNS-PY-001`: Python to research marker; Rust evaluator is grading truth.
32. `KNS-APP-001` (P1): desktop app for anyone on the Rust core; Atlas
    default with plain words (first pass landed 2026-09-20), single binary
    serving the UI, Tauri shell linking the crate, keychain, first run,
    ACP agents, installers. Drives `KNS-RUST-001`.
31. `KNS-JAIL-001` (P1) - implemented 2026-09-20 on Knossos branch
    `fix/wire-delegation-constitution-ariadne` (commit 9f1c298), pending CI
    on Linux and macOS: child-process workspace jail, Landlock on Linux and
    Seatbelt on macOS, `KNOSSOS_CONFINE` policy, confinement level on every
    `Finished` and in `Outcome.residual_risk`; `knossos exec` for Field.

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
recoverable local inference appliance from a verifiable artifact, can install the
training devkit and complete a bounded fine-tune on a Tier 1 or 2 node, can pair
a second node into Mesh with one code, and every support claim maps to a retained
hardware record.

Knossos v1 is complete when a non-developer can install one Rust artifact, run a
bounded coding mission through any supported interface/engine, survive interruption,
and receive independently verified changes without the model or harness being able
to game completion.

The combined product is complete when that Knossos workflow runs on a Cameo
appliance after a one-time `cameo knossos install`, with no internet needed at
run time, survives the defined compute and process failures, and can be operated
through Field, served from the appliance's own origin and reached from a phone
over the operator's private network, with any ACP agent as a unit, without
weakening either product's policy or evidence model.

Anything less remains beta, regardless of test count or visual polish.
