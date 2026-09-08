# Production roadmap audit

Audit date: 2026-09-05. Scope: Cameo and its bundled `daedalus/knossos-rs` and
`daedalus/field` release trees. Baseline: the existing dirty working tree, preserved.
The separate `Downloads/daedalus` checkout was not modified.

**The full production roadmap is not complete and this checkout is not qualified
for a stable release.** Passing software tests does not establish AMD execution,
power-loss recovery, artwork rights, or production security certification.

`PRODUCTIZATION_PLAN.md` is the current cross-product roadmap. The older
`CAMEO_PROJECT_PLAN.md`, `docs/remediation-plan.md`, `AUDIT_PLAN.md`, and
`docs/release-readiness.md` describe earlier scope or baselines. Their green labels
and historical counts cannot establish completion of the newer production gates.
In particular, both alleged literal `***` authorization-header defects from
`AUDIT_PLAN.md` are already fixed in the current source.

## Implemented before this audit

- Cameo: hardware detection, placement, authenticated daemon and `/v1` proxy,
  model acquisition/checksums, endpoint health/restart/eviction, session leases,
  metrics, container and ISO build scripts, guided offline installation,
  recommendation/setup preview, capability manifest, mesh request admission and
  pairing preview. Evidence: `core/`, `cli/src/main.rs`, `cameod/src/`,
  `contracts/`, `archiso/`, and executable Rust tests.
- Knossos: Rust CLI/serve/ACP, provider adapters, policy gates, tools, context
  budgets, mission journal and snapshots, proof invalidation, resume and uncertain
  action handling. Evidence: `daedalus/knossos-rs/src/` and its integration tests.
- Field: authenticated browser/WS control, scoped permission capabilities,
  environment allowlists, filesystem boundaries, bounded terminals/WS,
  event integrity, production/rehearsal separation, evidence-derived world state,
  policy/budget/delegation enforcement, durable routine subset, imported release
  package and asset gate. Evidence: `daedalus/field/server/`, `web/`, release tools,
  and `IMPLEMENTATION_LEDGER.md`.

## Completed during this audit

1. **Buffered proxy integrity and limits:** invalid status/framing, ambiguous
   length/transfer encoding, truncated lengths/chunks, missing terminal chunks,
   oversized headers, and responses over 32 MiB now fail rather than returning
   plausible successful output. Header cap is 64 KiB. Upstream gateway failures
   no longer disclose the private host, port, or OS error to consumers. Mid-stream
   I/O errors propagate; the relay does not invent a successful completion.
   Evidence: `cameod/src/proxy.rs`, `cameod/src/app.rs`; framing and socket tests.
2. **Markdown security:** replaced the regex renderer with `markdown-it` and
   `sanitize-html`, disabled source HTML, allowlisted elements/attributes, rejected
   unsafe URLs and credential-bearing links, and removed remote images. Added
   adversarial fixtures and CommonMark nested-list/reference-link coverage.
   Evidence: `daedalus/field/web/src/workspace/md.js` and Markdown security suite.
   This closes the parser-replacement residual in ledger DA-002.
3. **Nested ignore policy:** maintained Git ignore parser, scoped nested rules,
   excluded-parent semantics, immediate policy reload, `.env*` hiding, and
   no-follow/regular-file/size checks for ignore files. Unreadable policy fails
   closed. Evidence: Field workspace-path module and regression suite. This closes
   the nested-ignore residual in DA-004; platform race resistance is still limited.
4. **Initial bundle budget:** lazy-load City and its parser/sanitizer. Initial
   JavaScript is 92.21 kB gzip; deferred City is 95.00 kB gzip; CSS is 26.09 kB gzip.
   This meets the proposed JS/CSS transfer budgets, not the entire performance gate.
5. **ISO publication integrity:** publishing now depends on successful build jobs
   and requires both ISO artifacts. Previously `always()` allowed partial releases
   following failed builds. Workflow execution on GitHub remains to be observed.
6. **Platform checks:** Field CI now schedules Linux, Windows, and macOS suites
   and builds. Only Windows results are observed in this audit.
7. **Capability distribution:** `cameo capabilities` prints the same typed JSON
   used by daemon discovery, newly built ISOs stage that manifest, and generated
   `docs/capabilities.md` is checked for drift in CI. Site and console generation
   remain open; this does not close all of CAM-SCP-001.

8. **Durable endpoint recovery:** exclusive OS ownership lock; versioned SHA-256
   snapshots; write/sync/rename commits; persisted desired model settings, observed
   health and restart attempts. Start/eviction/stop persist intent before process
   mutation. Restart replans from current hardware, verifies model bytes and probes
   the port; stale records report recovery_required, never ready. Recovery retries
   are bounded. See supervisor-recovery.md for exact limits and compatibility.
9. **Browser logout:** ten-minute bootstrap expiry, session revocation, clearing
   the cookie, terminating existing sockets, and stopping browser reconnects.
   Permission capabilities for running harnesses are independent. Security tests
   and real WebSocket revocation pass; full browser workflow QA remains open.
10. **Routine restart behavior:** durable schedule claims suppress repeated slots
    after restart/backward clocks; unfinished runs become failed. UTC next-run,
    current owner and bounded history appear in the UI. Tests cover leap years,
    impossible dates, numeric steps and restart. Claims are at-most-once, so a crash
    after a claim can skip a run; queue/replace/timezone expansion remains open.
11. **Budget and capacity admission:** reserve before spawn, narrow allocations to
    campaign remainder, retain unknown final bills, reject exhausted resume paths,
    and replay monotonic totals without duplicate charges. Registry checks global,
    endpoint and campaign session caps. Draining process handles retain capacity;
    old child callbacks cannot corrupt a replacement. Both adapters pass a real
    local Node child pause/resume regression. Provider-enforced monetary ceilings,
    unknown-bill reconciliation and durable queue admission remain open.
    Ledger follow-up: verification/reinforcement inherit source campaign admission,
    reinforced sessions join assignment settlement, and restarted harnesses recheck
    admission, restore their original deadline and rotate permission capability.
    Full Field regression suite passes after these changes.
12. **Support report:** cameo doctor --bundle creates a previewed allowlisted JSON
    report, explicitly labels untested hardware/inference and never overwrites an
    existing file. Export/no-overwrite smoke passed on Windows. It is not a backup.

## Roadmap disposition

“Partial” means an implementation exists but does not meet every acceptance item.
“Unverified” means no complete acceptance evidence was established here; it does
not assert that every underlying primitive is absent. No broad issue is marked
complete merely because a similarly named module exists.

| Roadmap issue(s) | Disposition | Evidence and remaining work |
|---|---|---|
| KNS-SEC-001 | Partial | `security.js`: bootstrap/auth/capabilities implemented; expiring bootstrap, browser logout and socket revocation now implemented; CLI logout/re-enrollment and full lifecycle acceptance remain. |
| KNS-SEC-002 | Implemented locally | Security and WS suites pass origin/host/auth gates; deployment-edge review remains a release gate. |
| KNS-SEC-003 | Implemented locally | Maintained parser + sanitizer added here; malicious content fixtures and production build pass. |
| KNS-SEC-004 | Partial | Nested ignores closed here; Unix/macOS execution and stronger reparse/TOCTOU guarantees remain. |
| KNS-SEC-005 | Partial | Environment, terminal bounds, capability and WS tests pass; platform process-tree matrix must execute. |
| KNS-POL-001 / 002 | Partial | `permission-policy`/adapter tests pass; shell/egress cannot be treated as a complete OS sandbox. |
| KNS-POL-003 / 004 | Partial | Registry/budget/delegation tests pass; durable reservations, unknown-cost holds, monotonic accounting and registry capacity admission now implemented; provider ceilings, cost reconciliation and full delegation/queue acceptance remain. |
| KNS-DATA-001 / 002 / 003 | Implemented locally | Eventlog, replay, world, lifecycle and synthetic separation tests pass; real-provider acceptance remains. |
| KNS-RTN-001 | Partial | Durable cron subset and skip-overlap implemented; durable minute claims, UTC next-run/history UI and restart tests added; queue-one and persisted cooldown added with restart/configuration/claim tests; replace and parallel overlap added with cancel/concurrent fixtures; non-UTC zones and multi-mission fairness remain open. |
| KNS-REP-001 | Implemented locally | Field exists in nested Knossos release tree and CI/package tools; dirty submodule changes are not a published release commit. |
| KNS-LOOP-001 | Partial | `mission.rs` contract/state and runtime wiring exist; full amendments and operator contract UI acceptance unverified. |
| KNS-LOOP-002 / 003 / 005 | Unverified as complete | Gate, result and memory primitives exist; progressive autonomy, grouped approvals, and accepted-capability metrics need end-to-end qualification. |
| KNS-LOOP-004 | Partial | Task, Serve, ACP and REPL checkpoint wiring exists; fresh-process CLI, Serve and ACP continuation tests pass with policy/environment checks. Accept/revise/revert lineage, unsupported tool capsules, uncertain-action reconciliation and live-provider recovery remain. |
| KNS-LOOP-006 | Partial | Journal/snapshot/replay and live wiring tested; schema migration and power-failure qualification remain. |
| KNS-LOOP-007 | Partial | Context budgets/compaction tested; full semantic invariant suite and long-mission proof remain. |
| KNS-LOOP-008 / 009 / 010 | Partial | Tool dispatch, planning, cancellation and recovery primitives; complete action protocol, plan DAG and resource-lock scheduler unverified. |
| KNS-LOOP-011 / 012 | Partial | Proof invalidation/Oracle and delegation exist; content-bound independent proof and isolated-writer acceptance remain. |
| KNS-LOOP-013 / 014 | Partial | Provider/Cameo adapters and mock tests pass; strict cloud/local conformance, complete cost ceilings and crash matrix remain. |
| KNS-ENV-001 | Unverified as complete | Reproducible mission environment export, fingerprints and supported-platform setup matrix need acceptance evidence. |
| KNS-GIT-001 | Partial | Git evidence and dirty-worktree refusal exist; managed restore preserving staged/untracked content remains open. |
| KNS-EXT-001 | Partial | MCP/hooks/tools exist; complete trust manifest/SDK/registry conformance not qualified. |
| KNS-ID-001 | Partial | Scoped session capabilities and environment scrubbing exist; opaque secret broker/keychain/rotation remain. |
| KNS-TRUST-001 | Partial | Policy and injection fixtures exist; DNS/redirect-aware egress enforcement remains explicitly open. |
| KNS-SCHED-001 | Unverified as complete | Routine/registry scheduling exists; full durable multi-mission fairness/lock/restart gate not established. |
| KNS-VIS-001 | Partial | Allowlisted browser pane exists; isolated browser tooling and revision-bound visual proof remain. |
| KNS-HAR-001 / KNS-EVAL-001 | Partial | Rust, Python reference and suite-audit tooling exist; signed-off parity decision and non-saturated hard/live results remain. |
| KNS-UX-001 / 002 / 003 | Partial | Naming, World/City/Senate and evidence-derived gameplay exist; full onboarding/responsive/Atlas acceptance unverified. |
| KNS-A11Y-001 | Unverified | No complete WCAG 2.2 AA operator-workflow audit established. |
| KNS-PERF-001 | Partial | Asset/stress gates and JS/CSS budgets pass; real-browser LCP/CLS/interaction evidence remains. |
| KNS-REL-001 / ORG-GOV-001 | Partial | Packaging/CI/checksums exist; Field asset rights are owner-attested Apache-2.0 and the release asset audit passes; signed artifacts, governance and clean tagged publication remain. |
| CAM-SCP-001 | Partial | Typed manifest generates CLI, docs, ISO, site table/JSON, and console Capabilities panel; certification evidence remains. |
| CAM-SEC-001 | Partial | HTTP/auth/rate-limit/secret regression tests pass; independent review, TLS device binding and platform qualification remain. |
| CAM-API-001 | Partial | Proxy defects closed here; gateway now rejects unadvertised OpenAI parameters and caps max_tokens to served context; silent-disconnect cancellation, fairness and live pinned-backend conformance remain. |
| CAM-ENG-001 | Partial | Engine schema, capabilities and leases exist; malformed v1 cannot provision; advertised native-tool/context/token/byte ceilings are now applied by the Knossos adapter; live pinned-backend conformance remains. |
| CAM-MDL-001 | Partial | SHA-256 downloads/cache lifecycle exist; content-addressed catalogue, model-license policy, quarantine/offline bundle and full supply-chain gate remain. |
| CAM-STATE-001 | Partial production blocker | Versioned, checksummed, locked endpoint intent snapshots and bounded restart recovery now implemented; offline endpoint-intent check/backup/restore now pass; durable lease ownership and v1-to-v2 migration now pass; bounded orphan expiry now passes; session identity recovery now passes; process adoption and hardware/power recovery remain. |
| CAM-STO-001 | Partial | Offline guided installer/persistence exist; encryption, quotas, verified backups/restores and granular factory reset remain. |
| CAM-UPD-001 | Partial | The installer provisions A/B roots and separate persistent state; signed application bundles are built deterministically and the host journal stages a boot trial, health-commits it, restores incompatible state before fallback, and recovers the layout-rename crash window. Full-OS payload construction plus observed Linux interruption/signing/hardware evidence remain. |
| CAM-ISO-001 | Partial | Full/lite builds and QEMU step exist; release integrity fixed here. Reproducibility, installation and power-interruption tests remain unexecuted here. |
| CAM-HW-001 | Partial/external evidence | Detection/recommendation and Phase-1 runbook exist; no certified hardware matrix or calibrated fit results established. |
| CAM-MESH-001 | Partial | Pairing and request scheduler preview exist; manifest explicitly says no mTLS or sharding; discovery, mixed-version/failure/owner-reclaim qualification remain. |
| CAM-OBS-001 | Partial | Prometheus/health surfaces exist; allowlisted doctor JSON export and no-overwrite smoke pass; retention, alerts, verified restore and support acceptance remain. |
| CAM-PERF-001 | Unverified | No hardware-bound capacity/power/thermal benchmark qualification established. |
| CAM-UX-001 | Partial | Recommendation/setup CLI and console exist; three-profile transactional setup/proof/rollback and accessible offline UX remain. |
| CAM-KNS-001 | Partial | Bundling and Cameo adapter exist; real offline appliance/reboot/eviction/interrupted-stream run remains. |
| CAM-REL-001 | Partial | Build/checksum/publish workflows exist; signatures, SBOM, component/model provenance and hardware evidence remain. |
| RC-001 | Blocked | Depends on the outstanding production implementation and external qualification above. |

## Verification and limits

- Cameo: baseline 258 Rust tests passed; **293 test results pass after changes**
  and were revalidated on 2026-09-07. Full workspace test, format and strict
  all-target Clippy checks all pass.
- Knossos: 568 Rust test results passed on 2026-09-07, including
  harness/serve/mock integration.
  Live-engine tests can return early without configured providers: this is **not**
  cloud/local live-provider qualification.
- Field: all scripted suites passed, including 100,004-event stress fixture;
  production build passed. Package installation reported zero vulnerabilities;
  this is an npm audit result, not independent security certification.
- YAML parsing passes for all three changed workflows. The capability docs drift
  check passes, and actual CLI output matches the canonical JSON manifest.
- `npm run release:audit` now passes: the six Field images are recorded as
  owner-generated Apache-2.0 artwork (attestation 2026-09-06). The audit still
  fails closed if a later asset is unknown or unverified.
- Local text logs are retained under `target/production-audit/` (ignored build
  output, not portable release attestations).
- No model download, real AMD inference, Linux ISO build/install, destructive disk
  operation, update interruption, new CI run, deployment or publication occurred.
- Field artwork provenance is the owner's 2026-09-06 attestation, not inferred
  from filenames. Signing keys and hardware evidence remain separate gates.

## Remaining implementation order

1. Finish the P0 security/rights/clean-release gates and run the new OS matrix.
2. Finish durable supervisor reconciliation, lease persistence and restore before transactional
   update work; never adopt or kill unknown processes by PID alone.
3. Complete serving capability negotiation and cancellation against a pinned
   runtime, then mesh mTLS enrollment/revocation and failure qualification.
4. Complete model/storage transactions and the full-OS update payload; execute the configured signed release path on Linux.
5. Finish the Knossos mission/environment/delivery gaps above, UI/accessibility,
   and hardware/live-provider/upgrade RC acceptance with retained evidence.

Parser implementation references: [markdown-it](https://github.com/markdown-it/markdown-it),
[sanitize-html](https://github.com/apostrophecms/sanitize-html), and
[ignore](https://github.com/kaelzhang/node-ignore). These inform the implementation;
the local regression tests are the evidence for this checkout.
