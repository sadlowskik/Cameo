# Production completion and final-testing plan

Updated 2026-09-07. Owner: coordinating agent. Execution is active.

## Scope and evidence rules

Finish the full `PRODUCTIZATION_PLAN.md`, including Workstream H1-H16, all 61
issues in section 15, P0/P1 gates, and the cross-product release-candidate workflow.
`IMPLEMENTATION_LEDGER.md` records implementations and their limits;
`docs/production-audit.md` is the current crosscheck. Older green roadmap labels
do not override the newer acceptance requirements. Final testing means a complete
candidate can enter the prescribed acceptance matrix; it does not mean a local
test suite alone certifies hardware or a stable release.

For every closure retain: requirement, changed production path, regression or
acceptance fixture, actual command/outcome, supported platform, limitations,
compatibility/migration impact, and rollback procedure. Missing, skipped, mocked,
and failed evidence remain distinct. Do not close a whole issue for one helper.

## Team and file ownership

| Owner/model | Scope | Detailed plan | First active job |
|---|---|---|---|
| Sol / gpt-5.6-sol | Cameo core, daemon, CLI, installer/update scripts, contracts | [Cameo](completion-cameo.md) | H12 update transaction and signed offline bundle |
| Terra / gpt-5.6-terra | Nested `daedalus/knossos-rs` runtime and tests | [Knossos](completion-knossos.md) | Mission environment verification and reproducibility |
| Luna / gpt-5.6-luna | Nested `daedalus/field` server, UI and tests | [Field](completion-field.md) | Audit remaining policy/scheduling/operator gaps and implement dependency-first closure |
| Coordinator | Shared roadmap/ledger, CI/release integration, cross-product contracts, review and qualification | This plan | Complete issue coverage, release checks, integrate and challenge evidence |

Agents report their intended files before editing. Sol alone may update root
Cargo manifests/lockfile for its job. The coordinator owns shared ledger/audit
and workflows. Cross-lane API or schema changes require a handoff describing
version, migration, old-client behavior and contract fixtures. Work occurs in the
existing dirty tree; preserve unrelated edits. The sibling Downloads/daedalus
checkout is a donor and is not a release input or editing target.

## Dependency waves

1. **Reconcile and protect the baseline.** Map every issue to current production
   wiring and explicit missing acceptance evidence. Preserve dirty files; identify
   generated output and release inputs. Establish locked build/test gates. Exit:
   no unowned issue, no assumed completion, and reproducible local test commands.
2. **Authority and durable work.** Close remaining A/B policy bypasses, identity
   lifecycle, delegation inheritance and bounded costs. Finish mission state,
   action identity, proof invalidation, environment fingerprints and safe Git
   delivery. Finish routines/queue fairness and crash convergence. Exit: denied
   operations cannot execute through any adapter/resume/delegation path; restarts
   preserve authority and truth without duplicate effects or lost user changes.
3. **Appliance transactions and serving.** H9 ownership/recovery feeds H12 update
   drain/snapshot/rollback; H10 model provenance and H11 storage feed first-run
   setup. Complete H8 feature negotiation/QoS/cancel and shared engine contract.
   Exit: signed updates interrupted at each boundary recover the prior bootable
   system or complete health promotion; model/state preservation and black-box
   serving assertions hold. Mock tests precede Linux VM and real hardware tests.
4. **Distributed and operator integration.** H13 mTLS enrollment/revocation,
   discovery, capacity expiry, streaming/cancel, affinity/reclaim and partitions;
   H1 claims derive from capabilities. Finish mission contract/result/approval UI,
   Rome/Atlas/responsive states, onboarding and accessible workflows. Exit:
   three-node fault tests and independent browser/a11y/performance evidence.
5. **Release inputs and supply chain.** Locked platform builds, editor/Field
   packaging, shell/container/ISO tests, advisory/license/secret checks, SBOMs,
   signatures, compatibility/component manifests and reproducibility. Resolve
   actual artwork rights and governance ownership. Exit: clean protected tagged
   sources yield a complete verifiable artifact set, with no manual copying.
6. **Final testing and candidate qualification.** Execute clean-machine commands,
   cloud + generic local + Cameo live conformance with strict no-skip settings,
   long missions and provider switches, AMD classes, three-node mesh, OS/storage/
   GPU/network faults, update interruption, 24-hour soak and seven-day candidate
   dogfood. Exit: every P0/P1 and H16 item has release-bound evidence. Certification
   only applies to the actual tested combinations. Stable publishing remains a
   separate consequential action, not a side effect of running tests.

Waves overlap only where dependencies are satisfied: Field and runtime work can
proceed while Cameo transactions are built. Integration review follows every
coherent job; agents then take the next open job in their lane. Do not wait for all
three lanes to finish before reviewing a completed change.

## Required acceptance families

| Family | Evidence required before closing |
|---|---|
| A / security | HTTP/WS bootstrap/logout/re-enrollment; origin/host/CSRF; malicious Markdown; symlink/junction races; child env/process tree across supported OSes |
| B / authority | Same policy across adapters; deny-path fixtures; session/routine/campaign costs; unknown billing; admission/delegation/resume; enforced egress and secret scopes |
| C / truth | Production/rehearsal isolation; independent content-bound verdicts; assignment convergence; real Git checkpoint/rollback; durable event corruption/replay matrix |
| D / routines | Persisted claims/history; specified timezones/DST/overlap/misfires; restart/failure/cooldown; budget-bound concurrent schedules |
| E / runtime | Full contract-through-accept/revise/revert; all 14 loop issues; environment/Git/tool/identity/trust/scheduler/browser gates; parity/hard/live suites |
| F-G / Field | Rome/Atlas desktop/compact/mobile states; 30-agent crowding; keyboard/screen-reader workflows; WCAG 2.2 AA review; measured LCP/CLS/interaction and bundle budgets; asset provenance |
| H / Cameo | Each H1-H16 gate in Sol's plan, including bootable update rollback, real AMD evidence and three-device authenticated mesh |
| I / releases | Supported OS/architecture builds; Python/Rust/ACP/serve/editor/Field checks; shell/container/ISO fixtures; scans/SBOM/signatures; compatibility and governance; clean artifact-only walkthrough |
| RC | All above plus version-to-version upgrades, 24-hour hardware soak and seven-day artifact-only dogfood; no silently skipped qualification |

## Validation and review protocol

- Run focused regression fixtures first, then relevant full suite, strict lint,
  format and production build for each changed lane. Coordinator reviews actual
  diff and tests for coverage of failures, not only happy-path helper behavior.
- Cameo: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
  `cargo test --workspace --locked`, `cargo build --workspace --release --locked`.
- Knossos (nested crate): format, all-target locked Clippy/tests, locked release
  build, ACP/serve and editor checks. Live tests must use strict configuration;
  an absent provider is missing evidence, never a pass.
- Field: `npm ci`, `npm test`, `npm run build`, browser/a11y/performance acceptance,
  and `npm run release:audit`. The last command now passes after the owner
  attested the six Field images; it must remain binding for any later asset.
- Cross-product: contract version/compatibility fixtures plus actual Field to
  Knossos to Cameo workflow. Repeat broad suites when integrated code or contract
  changes justify it. Local ignored logs are diagnostic evidence, not signed
  release attestations. CI configuration is not evidence of a completed CI run.

## External inputs and executable limits

WSL inventory failed with `REGDB_E_CLASSNOTREG` on this host; installed Linux and
systemd are not currently available for qualification. No Docker executable was
found by the same inventory. Continue local implementation and fault fixtures;
use a configured Linux/AMD environment for the corresponding acceptance gates.

The six Field images are owner-attested original artwork under Apache-2.0; that
rights gate is no longer blocked. Release signing identities, ownership/governance
decisions, protected repository settings, cloud-provider credentials/cost
authorization, owned AMD hardware and soak time must still be supplied or verified
when those gates become executable. Do not invent credentials, certification or
completed external runs.

## Completion accounting

The companion [issue checklist](completion-issues.md) preserves every numbered
roadmap issue with its owner. Detailed lane plans refine scope and tests without
removing requirements. Mark an issue complete only after the coordinator checks
each acceptance item against retained evidence and records it in the ledger.
The 2026-09-07 evidence reconciliation distinguishes 6 issues implemented locally,
47 partial, 7 unverified as complete, 0 open production blockers, and the blocked
release candidate; none is yet release-accepted. This replaces the earlier generic
"Open acceptance" label without weakening the release evidence rule.
The whole goal remains active until the full final-testing/candidate scope is
proven; neither a plan nor three completed first jobs establishes completion.

## Active integration review

| Job | Current review result | Required next evidence |
|---|---|---|
| Shared release gates | Four workflow YAML files parse; ISO Docker option ordering corrected; 61 unique issue IDs accounted for; capability docs/site/JSON generation now drift-checked | Actual CI matrix, container and ISO runs |
| Sol H12 | A/B install, signed application-bundle producer/consumer, host apply, health promotion, automatic fallback, journal recovery, and schema-aware state restore have local fixtures | Full-OS component payload, observed Linux interruption at every journal phase, signing-secret run, and hardware soak |
| Terra environment | Streaming SHA-256 environment evidence plus task, Serve, ACP, and REPL checkpoint wiring exist; fresh-process CLI, Serve, and ACP continuation tests preserve the supported mission contract/policy/quota | Accept/revise/revert lineage, MCP/delegation and preview capsules, uncertain-action reconciliation, schema/power-failure coverage, and live-provider recovery |
| Luna pagination and routines | Bounded traces, queue-one recovery, and replace/parallel overlap are implemented and unit-tested | Timezone/DST expansion, multi-mission fairness, browser acceptance and broader runtime integration |
| Coordinator discovery | Malformed v1 cannot provision; gateway and Cameo adapter now enforce advertised features/limits | Live pinned-backend and cloud black-box conformance |

Agent reports are inputs to review, not automatic acceptance. These jobs remain
open until the coordinator checks the requested evidence and production wiring.

ChatGPT/Codex usage credits were exhausted on 2026-09-07. This coordinator
continues the same completion goal: A/B host apply is real code, schema-aware
rollback is in fixtures, and signed manifests now require the full OS/runtime
identity set. Hardware, observed Linux interruption, ISO signing-secret runs,
and soak stay blocked on this host.
