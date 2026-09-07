# Field completion lane

Date: 2026-09-05. Scope: `daedalus/field` only. This is an implementation lane
record; the central ledger, audit, and completion plan remain root-owned.

## Evidence-backed residual inventory

| Priority | Area | Concrete residual in this checkout | Acceptance gate | Dependency |
| --- | --- | --- | --- | --- |
| P1 | A1/A2 security lifecycle | `server/src/security.js`, `index.js`, and `ws.js` cover bootstrap, cookies, logout, origin/host, and revocation; full browser lifecycle and deployment-edge review remain unobserved | Fresh profile bootstrap, restart invalidation, logout socket closure, and configured remote/TLS mode pass on supported OSes | OS/browser and deployment qualification |
| P1 | A4/A5 containment/process | `server/src/workspace-path.js` and terminal policy pass maintained Windows fixtures; Unix/macOS symlink/reparse race and process-tree matrix remain unexecuted | Linux/macOS/Windows junction, symlink, device, tree-kill, timeout, and backpressure fixtures pass | Supported release hosts |
| P1 | A6 child environment | `server/src/child-env.js` and `server/test/child-env.test.mjs` cover allowlists/canaries; opaque credential broker and keychain rotation remain absent | Provider adapters prove unrelated secrets cannot be read, and broker lifecycle is qualified | Provider/keychain integration |
| P1 | B1/B2 policy parity | `server/src/harness/registry.js`, `server/src/harness/tools.js`, and `server/test/permission-policy.test.mjs` enforce local policy; narrower delegated loadout and OS-level shell/egress boundary remain incomplete | Anthropic and Knossos conformance fixtures show equivalent role, path, tool, subprocess, and domain outcomes | Live provider fixtures |
| P1 | B3/B4 resources | Registry budgets, unknown holds, campaign inheritance, generation controls, and caps are implemented; provider billing reconciliation, durable queue admission, and complete delegation-depth acceptance remain | Provider-reported ceilings, duplicate/retry/restart accounting, queue fairness, and child-depth limits pass | Provider telemetry and scheduler qualification |
| P1 | C1/C2 truth | Projection and synthetic partition are evidence-derived and tested; live-provider evidence and complete UI provenance inspection remain open | Skipped proof never verifies; production/rehearsal costs, traces, and maturity remain isolated in live runs | Live provider and browser QA |
| P1 | C3/C4 lifecycle | Assignment terminal events and managed Git record-only checkpoint semantics exist; full searchable/grouped trace UX and managed restore workflow remain incomplete | Replay repairs missing terminal states; checkpoint/rollback behavior is explicit, reviewed, non-destructive, and conflict-reporting | Git/browser acceptance |
| P1 | C5 event queries | `server/src/api.js` now validates bounded pagination for events, subject traces, and campaign traces; campaign membership still derives through a full-log scan | Route tests cover invalid input, second page, terminal page, stable `head`, and `nextFrom`; indexed campaign lookup remains a follow-up | Existing EventLog backends; future index design |
| P1 | D1/D2 routines | Durable enablement, UTC claims, restart interruption, skip, queue-one, replace, and parallel overlap are implemented in `server/src/routines.js`; timezone/DST expansion remains unsupported | Supported subset is documented and tested; expanded timezones require dedicated DST fixtures | Scheduler design |
| P1 | D3 routine safety | Confirmation, budget/policy validation, debounce cancellation, and self-trigger suppression pass; broader loop detection and queued-run bounds remain open | Trigger mutation cannot recursively self-start and routine resource policy is enforced through restart/failure cases | Filesystem event qualification |
| P1 | F1/F2 naming/world | `web/src/App.jsx`, `theater/TheaterMode.jsx`, and field renderer use current Field lenses and evidence-derived state; 30-agent collision/formation, filters, and all operation outcome affordances need acceptance | Dense roster selection/focus/team-role-state filters and complete/failed/interrupted/attention feedback work without hover dependence | Browser interaction QA |
| P1 | F3/F4 themes/responsive | Rome/Atlas controls and responsive CSS exist in `web/src/styles`; Atlas landmark contrast and compact/mobile operational layouts lack measured acceptance | Every state is checked in both themes at desktop/tablet/mobile widths with readable labels and touch targets | Browser/device matrix |
| P1 | F5 accessibility | ARIA/live regions and modal focus hook exist; no complete WCAG 2.2 AA audit is retained | Keyboard, 200% zoom, high contrast, reduced motion, screen reader, automated scan, and manual workflow checklist pass | Browser/assistive tech |
| P1 | F6 onboarding/states | Capital/rehearsal flows exist; guided first launch, sample repository, searchable help, and all disconnected/corrupt/budget states remain incomplete | New user safely rehearses then dispatches real work within the roadmap time gates | Product/browser QA |
| P0 | G1 rights/assets | Owner attested 2026-09-06 that the six Field images were generated for this repository; manifest records Apache-2.0 and APPROVED; `npm run release:audit` passes | Future assets must still enter BLOCKED until creator/license/review are filled | Owner attestation recorded; signing/publish remain separate |
| P1 | G2/G3 delivery | Lazy City and local assets meet initial transfer budgets; AVIF/WebP density, immutable caching, ETags, and all static security headers lack complete release evidence | Measured image delivery and static response checks pass on supported browsers/offline mode | Browser/server profile |
| P1 | G4 performance | `scripts/audit-assets.mjs` checks bundle size; no retained real-browser LCP/CLS/interaction or sustained 30-agent trace exists | Release profile records agreed throttled-browser metrics and bounded stream memory/control latency | Supported browser baseline |
| P1 | E Field integration | `server/src/orchestration` emits mission/campaign evidence and UI shows traces; end-to-end ceremony, accepted-capability metrics, and long mission resume/provider-switch proof remain unqualified | Safe mission reaches useful action quickly, preserves context, exposes evidence/cost/uncertainty, and never verifies without independent proof | Knossos/live provider integration |
| P1 | I1 CI/release | Field CI/package hooks exist under `../.github/workflows` and `release/package_field.py`; observed cross-platform jobs, live-provider protected suites, SBOM/signing, and clean tagged publication remain open | Windows/Linux/macOS Field jobs, security/build/a11y checks, provenance, and signed package evidence are retained | Protected CI runners/credentials |

## Selected implementation and verification

The completed local slice is C5 bounded event querying. `parseEventPage` in
`daedalus/field/server/src/api.js` rejects invalid cursors and limits. Event,
subject-trace, and campaign-trace responses are capped and return `head` plus a
continuation cursor only when a lookahead row exists. `EventLog.bySubject` in
`server/src/store/db.js` applies the same cursor semantics to SQLite and JSONL.
`web/src/net/client.js` accepts cursors, and `web/src/traces/TracesMode.jsx`
offers an explicit “Load more events” action so existing traces do not silently
truncate at the page boundary.

`server/test/api-pagination.test.mjs` covers parser rejection, all three real
API routes, second pages, terminal pages, filtering, and SQLite/JSONL parity.
The full Field command passes, including the 100,004-event stress fixture. The
Vite production build passes (1,912 modules; 92.49 kB gzip initial JS and
26.09 kB gzip CSS). These results required the approved Windows process-launch
permission for the existing process-generation and esbuild startup paths.

Artwork rights for the six current Field images are owner-attested. Live
providers, OS matrices, real-browser accessibility and performance, and release
signing remain external gates. No claim of completion for those gates is made
by this local slice.

## Coordinator overlap review, 2026-09-06

Replace interrupts the active session through `registry.command('cancel')` and
starts the new run. Parallel admits concurrent sessions and records
`activeRunIds` for restart interruption. Cooldown still rate-limits new starts.
`server/test/routines.test.mjs` covers both policies; queue-one recovery tests
still pass. The full Field scripted suite also passes after this slice.
Timezone/DST and multi-mission fairness remain open.

## Coordinator queue review, 2026-09-06

Queue-one now persists one pending item and an authority hash, restores unclaimed
work with its cooldown, claims before dispatch, and reconciles claimed-but-uncertain
work without replay. Config changes invalidate the queued authority. The new
server/test/routine-queue-recovery.test.mjs covers replay, timer overlap, disable,
configuration changes, persistence failure and failed budget admission. Full suite
and production build pass; final configuration-guard changes also pass the repeated full suite.
The ledger records exact compatibility limits and remaining scheduling work.
