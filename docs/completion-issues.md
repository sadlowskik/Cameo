# Complete production issue checklist

Updated 2026-09-06. All 61 issue IDs from PRODUCTIZATION_PLAN.md section 15.
Open acceptance does not mean no code exists: previous implementations and local
closures remain recorded in IMPLEMENTATION_LEDGER.md. It means the full issue
acceptance has not been signed off by this coordinated review. Lane plans must
identify concrete production wiring, residual tasks, and executable acceptance
checks before a row can become complete. No issue is omitted or waived.

| Issue | Roadmap deliverable | Owner | Acceptance state |
|---|---|---|---|
| KNS-SEC-001 | Field authentication and one-time browser bootstrap. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-SEC-002 | Origin/Host/CSRF/WebSocket enforcement. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-SEC-003 | Markdown sanitizer and security headers. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-SEC-004 | Canonical filesystem containment and sensitive-file policy. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-SEC-005 | Child environment, terminal bounds, and WebSocket backpressure. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-POL-001 | Unified cross-adapter policy object. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-POL-002 | Read/write/tool/environment scope enforcement. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-POL-003 | Session/routine/campaign budget enforcement. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-POL-004 | Concurrency and delegation enforcement. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-DATA-001 | Event provenance and production/synthetic separation. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-DATA-002 | Evidence-derived maturity and verification. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-DATA-003 | Assignment lifecycle and trace filtering. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-RTN-001 | Durable routine state and scheduling correctness. | Luna (Terra for runtime enforcement) | Open acceptance; skip/queue-one/replace/parallel locally tested; timezone/DST and fairness remain |
| KNS-REP-001 | Import Field and normalize repository/release structure. | Coordinator | Open acceptance; see ledger and lane plan |
| KNS-LOOP-001 | Mission contract schema, UI, amendments, and persistence. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-LOOP-002 | Progressive autonomy router and execution modes. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-LOOP-003 | Meaningful progress model, grouped approvals, and result object. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-LOOP-004 | Accept/revise/revert, continuation, recovery, and mission lineage. | Terra (Luna for operator integration) | Open acceptance; CLI persist/resume exists; serve/ACP restore remain |
| KNS-LOOP-005 | Accepted-capability memory and adoption metrics. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-LOOP-006 | Versioned `MissionState`, append-only journal, snapshots, and migrations. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-LOOP-007 | Context compiler, token accounting, semantic compaction, manifests, and invariant validator. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-LOOP-008 | Provider-neutral action/result protocol and artifact rehydration. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-LOOP-009 | Plan DAG, evidence-based progress, replanning, and bounded stuck recovery. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-LOOP-010 | Capability-aware tool scheduler, safe parallel reads, locks, cancellation, and idempotency. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-LOOP-011 | Hash-bound proof obligations, invalidation, flaky-check handling, and final Oracle gate. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-LOOP-012 | Typed child contracts, isolated writers, ownership, handoffs, and independent reviewers. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-LOOP-013 | Provider capability negotiation, model/effort routing, prompt caching, and complete cost budgets. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-LOOP-014 | Crash recovery, deterministic trace replay, long-context tests, and cross-engine conformance. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-ENV-001 | Reproducible mission environments, fingerprints, resource limits, caches, and setup export. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-GIT-001 | Dirty-tree-safe worktrees, patch/commit/branch handoff, conflict handling, and explicit publication gates. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-EXT-001 | Namespaced tool/MCP/skill registry, trust manifests, lazy loading, SDK, and conformance kit. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-ID-001 | Opaque secret handles, scoped short-lived credentials, identity display, rotation, and redaction tests. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-TRUST-001 | Instruction trust classes, destination-aware egress, injection/SSRF defenses, and adversarial fixtures. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-SCHED-001 | Durable multi-mission queue, locks, fairness, restart recovery, and notification policy. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-VIS-001 | Isolated browser tooling, visual/accessibility evidence, artifact viewers, and revision binding. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-HAR-001 | Rust/Python parity matrix and product-runtime closure. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-EVAL-001 | Hard-suite scoring and honesty evaluation. | Terra (Luna for operator integration) | Open acceptance; see ledger and lane plan |
| KNS-UX-001 | Naming and onboarding pass. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-UX-002 | World crowding, completion, and metric provenance. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-UX-003 | Atlas redesign and responsive/mobile status experience. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-A11Y-001 | WCAG 2.2 AA closure. | Luna (Terra for runtime enforcement) | Open acceptance; see ledger and lane plan |
| KNS-PERF-001 | Asset cleanup, code splitting, caching, and budgets. | Luna (Terra for runtime enforcement) | Open acceptance; Field asset rights approved; real-browser LCP/CLS remain |
| KNS-REL-001 | CI, release matrix, VSIX/Field artifacts, SBOM, and signing. | Coordinator | Open acceptance; see ledger and lane plan |
| ORG-GOV-001 | Product licenses, contributor policy, namespace ownership, history audit, maintainers, and protected publishing. | Coordinator | Open acceptance; see ledger and lane plan |
| CAM-SCP-001 | Freeze v1 scope, support labels, non-goals, and generated capability manifest; reconcile every conflicting public claim. | Sol | Open acceptance; docs/site/console/CLI/ISO generated; certification evidence remains |
| CAM-SEC-001 | Cameo control-plane, local/remote exposure, rate limits, credential scopes, and node trust re-audit. | Sol | Open acceptance; see ledger and lane plan |
| CAM-API-001 | Versioned OpenAI feature matrix, black-box conformance, streaming, cancellation, errors, QoS, and backpressure. | Sol | Open acceptance; gateway rejects unadvertised features locally; live pinned-backend remains |
| CAM-ENG-001 | `cameo-engine/v1` schema, capability negotiation, leases, request identity, and shared Knossos mock suite. | Sol | Open acceptance; discovery now applies feature/limit ceilings; live conformance remains |
| CAM-MDL-001 | Content-addressed model catalogue, provenance/license policy, quarantine, integrity, templates, lifecycle, and offline bundles. | Sol | Open acceptance; see ledger and lane plan |
| CAM-STATE-001 | Durable desired/observed supervisor state, atomic transitions, process adoption, migrations, and crash/power recovery. | Sol | Open acceptance; see ledger and lane plan |
| CAM-STO-001 | Destructive-safe installer, encryption option, storage separation, quotas, backup/restore, and granular factory reset. | Sol | Open acceptance; see ledger and lane plan |
| CAM-UPD-001 | Signed component manifests, drain/preflight, transactional update, health commit, offline update, and automatic rollback. | Sol | Open acceptance; signed verify and bound A/B fixture exist; host apply/bootable rollback remain |
| CAM-ISO-001 | Reproducible full/lite ISO, QEMU boot, bare installation, persistence, and installed/live posture tests. | Sol | Open acceptance; see ledger and lane plan |
| CAM-HW-001 | Certified/compatible hardware matrix, calibrated fit data, command flags, known-good manifests, workload-aware `RuntimeProfile` recommendation engine, and unsupported-path truth. | Sol | Open acceptance; see ledger and lane plan |
| CAM-MESH-001 | Cameo Link, discovery/pairing, mTLS identity, live capacity advertisements, one local proxy endpoint, model-aware routing, affinity, owner reclaim, model replication, partitions, and mixed-version tests. | Sol | Open acceptance; see ledger and lane plan |
| CAM-OBS-001 | Structured events, bounded metrics, privacy/retention, alerts, and redacted `cameo doctor --bundle`. | Sol | Open acceptance; see ledger and lane plan |
| CAM-PERF-001 | Capacity calibration, prediction error, concurrency, performance profiles, thermal/power safety, and benchmark protocol. | Sol | Open acceptance; see ledger and lane plan |
| CAM-UX-001 | First-boot workload selection, three-profile comparison, transactional one-action runtime/model setup, proof, rollback, endpoint lifecycle, accessibility, and offline console UX. | Sol | Open acceptance; see ledger and lane plan |
| CAM-KNS-001 | Scoped Knossos bundle and end-to-end offline appliance workflow with reboot, eviction, and interrupted-stream recovery. | Sol | Open acceptance; see ledger and lane plan |
| CAM-REL-001 | Signed Cameo artifacts, model/license provenance, SBOM, component manifest, hardware evidence, and support policy. | Coordinator + Sol | Open acceptance; see ledger and lane plan |
| RC-001 | Cross-product release-candidate qualification. | Coordinator | Open acceptance; see ledger and lane plan |
