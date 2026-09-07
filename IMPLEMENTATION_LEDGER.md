# Knossos + Cameo implementation ledger

Current scope: [PRODUCTIZATION_PLAN.md](PRODUCTIZATION_PLAN.md), including Workstream H.
This ledger records implementation evidence and residuals. The [production audit](docs/production-audit.md)
is a cross-check, not a substitute for this ledger. Dated updates below supersede
historical slice limitations where they explicitly close them.

This ledger is the release-facing record for product-plan implementation. Every landed
slice records its baseline, blast radius, security and usability effects, rollback, tests,
and residual risk. Existing user changes are preserved and assessed separately from new
work.

## Release baseline

- Cameo: 225 Rust tests passed before implementation.
- Knossos release integration (`Cameo/daedalus`): 539 Rust tests passed before implementation.
- Field: all scripted Node suites passed before implementation; the Vite production build
  passed after allowing its bundler to spawn.
- Working trees were already dirty. No reset, cleanup, or overwrite is permitted; the nested
  Knossos release tree is the integration baseline and the separate Knossos tree is a donor.

## DA-001 — Authenticate the Field control plane

Status: implemented and verified.

### Objective

Protect Field's file-write, terminal, session, permission, event, and WebSocket surfaces
from drive-by browser requests and unauthenticated local callers.

### Blast radius

- `daedalus/field/server/src/security.js`: new ephemeral control-plane authority.
- `daedalus/field/server/src/index.js`: HTTP bootstrap and request gate.
- `daedalus/field/server/src/ws.js`: authenticated WebSocket upgrade path.
- `daedalus/field/server/src/harness/{registry.js,permission-mcp.mjs}`: separate harness credential.
- Field security/WebSocket/registry tests and root test command.
- No persisted data, event schema, field configuration, workspace, or model API changes.

### Security assessment

- Replaces wildcard CORS and unauthenticated control APIs with a 256-bit one-time bootstrap,
  an HttpOnly/SameSite browser cookie, and a distinct 256-bit bearer token available only to
  the permission helper child.
- Enforces loopback peer, exact loopback Host, exact same Origin on mutations and WebSockets,
  JSON on the internal endpoint, constant-time secret comparison, and security headers.
- Fails closed when the permission helper has no internal credential.
- Tokens are process-ephemeral and added to event-log redaction inputs.
- Residual risk: an already-compromised process running as the same OS user can inspect child
  environments or control the browser. OS-user isolation and process sandboxing remain later
  release gates.

### Usability and compatibility assessment

- The normal web client needs no token handling; opening the printed bootstrap URL sets its
  session cookie and immediately removes the secret from the address bar.
- The existing Vite workflow receives one explicit `http://127.0.0.1:7748` exception only
  when launched through the server's `dev` lifecycle (or an explicit loopback-only
  `FIELD_UI_ORIGIN`). Production keeps an exact same-origin policy.
- Bare bookmarked URLs and unauthenticated scripts now receive an actionable 401. This is an
  intentional breaking change for an operator surface with code-execution authority.
- A bootstrap link is single-use. A new browser profile requires a Field restart in this
  first hardened slice; multi-device/operator enrollment remains out of scope for loopback
  Field.
- Harness permission prompts continue without browser-cookie coupling.

### Rollback

Revert only the files listed in the blast radius. No migration or data rollback is needed.

### Verification

- Targeted security-policy tests: passed, including real WebSocket rejection/acceptance.
- Full Field scripted suite: passed after implementation.
- Field production build: passed (1,829 modules; 308.29 kB application bundle,
  95.02 kB gzip).
- Manual HTTP bootstrap/API smoke test: passed (`401 -> 303 -> 200`, mutation without
  Origin `403`, same-origin request admitted).

## DA-002 — Sanitize rendered workspace Markdown

Status: implemented and verified. Update 2026-09-05: maintained Markdown parser and sanitizer replacement is complete; malicious-content fixtures and production build pass. The original assessment below is historical.

### Objective

Prevent untrusted workspace Markdown from creating executable or attribute-breaking links
inside the privileged Field operator origin.

### Blast radius

- `daedalus/field/web/src/workspace/md.js` and one renderer regression suite.
- No server API, persisted state, dependency, or Markdown source-file changes.

### Security assessment

- Raw HTML remains text-escaped.
- Links now permit only HTTP(S), fragments, and explicit relative/root-local paths.
- Quotes, controls, protocol-relative URLs, script/data protocols, and unsupported schemes
  render as their visible label without an anchor.
- External links receive both `noopener` and `noreferrer`; the server CSP remains a second
  defense against script execution and framing.
- Residual risk: this intentionally small renderer is not a complete CommonMark engine. Any
  future syntax extension must add adversarial rendering fixtures before release.

### Usability and compatibility assessment

- Ordinary documentation links, anchors, and relative repository links continue to render.
- `mailto:` and other custom protocols become plain text. That is a deliberate privilege-
  boundary tradeoff; users can copy the displayed label or source from the editor.

### Rollback

Revert the renderer and its test only. No data rollback is needed.

### Verification

- Targeted malicious-tag, protocol, quote-breakout, external-link, anchor, and relative-link
  fixtures: passed.
- Full Field suite and production build: passed.

## DA-005 — Bound terminal processes and WebSocket backpressure

Status: implemented and verified on the current Windows host; cross-platform process-tree
matrix remains a release gate.

### Objective

Prevent an operator terminal or stalled browser from exhausting Field and ensure cancellation
terminates descendant processes.

### Blast radius

- Field terminal start/output/kill path, WebSocket broadcast loop, and a pure policy test.
- No shell syntax, terminal response schema, or persisted event schema changes.

### Security assessment

- Defaults to four concurrent terminals, ten minutes, 2 MiB combined output, 20,000 lines,
  and 16,000 command characters.
- Terminates process trees on timeout, output overflow, and operator kill; slow WebSocket
  clients over 1 MiB buffered data are disconnected.
- Persists a credential-redacted command while showing the authenticated operator the exact
  workspace/cwd and entered command in the live terminal.
- Residual risk: shell parsing is still delegated to PowerShell/Bash for operator-entered
  commands. Agent shell policy needs a parser-backed allowlist before it can be considered a
  final authorization boundary.

### Usability and compatibility assessment

- Normal commands are unchanged and gain visible workspace/cwd context.
- Very long, noisy, or concurrent jobs now terminate with an explicit reason. Release UX must
  expose configurable limits before advanced workloads depend on larger values.
- Slow browser tabs reconnect through the existing client backoff after eviction.

### Rollback

Revert terminal policy/call-site and WebSocket buffer check. No data rollback is required.

### Verification

- Command validation/redaction and byte/line budget fixtures: passed.
- Full Field suite and production build: passed.
- Process-tree integration matrix: pending on macOS/Linux release hosts.

## DA-006 — Session-scoped permission capabilities and child environment isolation

Status: implemented and locally verified. Update 2026-09-05: direct harness restarts mint a fresh session-bound permission capability, invalidate the old token and register the replacement for log redaction. Both adapters recheck admission and rearm the original absolute deadline on actual process start. Live provider-adapter smoke tests remain pending.

### Objective

Prevent one harness from impersonating another at Field's permission bridge and stop child
processes from inheriting unrelated host credentials.

### Blast radius

- Ephemeral capability registry, permission bridge binding/revocation, dynamic log redaction.
- Claude and Knossos child-process environment construction and endpoint credential opt-in.
- No provider protocol, mission, event, or persisted configuration schema changes.

### Security assessment

- Each direct harness receives a random capability hashed in server memory and bound to one
  session ID; cross-session submissions fail and session termination revokes the capability.
- Capability values are dynamically added to event redaction and never grant browser access.
- Child environments now use an OS/runtime allowlist, the selected provider's credentials,
  and explicit endpoint `credential_env` opt-ins. Cloud, CI, registry, SSH-agent, and unrelated
  provider canaries are removed.
- Residual risk: the direct Claude CLI receives MCP configuration through its supported
  command-line mechanism, which may be inspectable by the same OS user. Moving capabilities
  to owner-only config files/opaque broker handles remains a later identity-broker gate.

### Usability and compatibility assessment

- Built-in Anthropic and local Cameo endpoints continue to receive their required environment.
- Custom authenticated OpenAI-compatible endpoints must declare `credential_env`; implicit
  leakage of every host variable is intentionally removed.
- Enterprise proxy-only environments may need a future explicit proxy allowlist; proxy URLs
  are not inherited by default because they can themselves contain credentials.

### Rollback

Revert capability and environment modules/call sites. Existing sessions must be restarted;
no persisted data rollback is required.

### Verification

- Capability minting, revocation, provider allowlist, and credential-canary fixtures: passed.
- Full Field suite: passed.
- Explicit cross-session route fixture and live Anthropic/Cameo adapter smoke tests: pending.

## DA-007 — Isolate and allowlist Field browser surfaces

Status: implemented and verified.

### Objective

Keep agent-observed or operator-entered URLs from turning the privileged Field origin into an
arbitrary protocol launcher or same-origin embedded browser.

### Blast radius

- Browser-pane URL validation/navigation UI, iframe sandbox, and CSP frame sources.
- `field.yaml`'s existing configured websites become the enforcement allowlist.

### Security assessment

- Allows complete HTTP(S) URLs only, rejects embedded credentials, and requires an exact
  configured hostname. Agent-discovered domains do not silently expand authority.
- Removes `allow-same-origin` and form submission from the iframe sandbox and suppresses
  referrers.
- CSP permits frames only from configured HTTP(S) origins; scripts in framed pages remain in a
  unique sandbox origin.
- Residual risk: a configured site can still present hostile content and run sandboxed script.
  A separate browser process/profile is the stronger future isolation boundary.

### Usability and compatibility assessment

- Configured docs/status pages continue to work when they permit framing.
- Arbitrary typed sites and newly observed agent domains now show a clear allowlist error until
  the operator intentionally adds the domain to `field.yaml`.
- Login/forms inside the embedded pane may no longer work; use an external browser for those
  workflows.

### Rollback

Revert the URL helper, BrowserPane changes, and `frame-src` generation. No data rollback.

### Verification

- Protocol, credential, URL completeness, port, and configured-domain fixtures: passed.
- Full Field suite and production browser build: passed.

## DA-008 — Establish Knossos MissionState and durable journal

Status: implemented and verified as an additive kernel module; runtime wiring is not yet
complete.

### Objective

Create the provider-independent source of truth required for durable missions before changing
the released execution loop or importing experimental donor behavior.

### Blast radius

- One additive Rust module exported by the Knossos library.
- No current Talos/provider/tool behavior changes in this slice; runtime wiring follows after
  the persistence invariants pass against the released baseline.

### Security and integrity assessment

- Safe mission IDs, symlink-resistant private workspace location, hash-chained append-only
  JSONL events, flushed/synced writes, and immutable sequence-numbered snapshots.
- Replay rejects sequence gaps, hash-chain changes, event tampering, unsupported schema
  versions, duplicate creation, and invalid transitions; one torn final line is recoverable.
- A consequential write increments workspace revision and invalidates proofs. Verified handoff
  requires every proof to pass against the exact current revision.
- Residual risk: SHA-1 is used for accidental/tamper evidence compatibility with the current
  crate, not as a keyed authenticity mechanism. A hostile same-user process can rewrite both
  data and hashes; signed exports and OS isolation remain release work.

### Usability and compatibility assessment

- The state is readable JSON/JSONL under `.knossos/missions/<id>` and independent of provider
  transcripts, enabling future offline/provider-switch resume.
- This additive slice creates no state until a caller opts into `MissionStore`; existing CLI,
  ACP, and serve behavior is unchanged.
- Future schema changes must provide explicit migration from version 1.

### Rollback

Remove the module export and source file. Existing mission directories are inert and can be
retained for re-import; no workspace source files are touched.

### Verification

- Replay/live-state equivalence, periodic atomic snapshot, transition guard, proof freshness
  and invalidation, torn-tail recovery, duplicate creation, and tamper tests: passed.
- Full released Knossos suite: passed (460 unit + 54 harness-loop + 2 live-engine +
  8 OpenAI-compat + 3 sandbox + 16 serve-loop tests; 543 total).

## DA-003 — Bound every Field request body and HTTP connection

Status: implemented and verified.

### Objective

Close the routine-toggle body-limit bypass and bound slow or excessive HTTP clients without
shortening long-lived permission decisions.

### Blast radius

- Shared JSON body reader used by the main API and routine toggle.
- Field HTTP server connection, header, request-ingestion, and keep-alive limits.
- One isolated parser test suite; no payload schema or persisted state changes.

### Security assessment

- Enforces the 4 MiB limit in wire bytes for all JSON mutation routes and a 10-second body
  ingestion deadline.
- Caps concurrent HTTP/WebSocket connections at 64 and bounds headers, request ingestion,
  and idle keep-alive time.
- Malformed, oversized, aborted, and stalled bodies fail with uniform codes and no stacks.
- Residual risk: per-route response deadlines and WebSocket send-buffer eviction are separate
  process-safety work and remain open.

### Usability and compatibility assessment

- Valid API payload behavior is unchanged. Empty bodies still decode to an empty object.
- Payloads above 4 MiB now receive `413`; slow uploads receive `408`. Field commands and
  configuration edits are expected to remain far below this boundary.
- Permission requests may still wait for the operator because only request ingestion is timed.

### Rollback

Revert the body helper, its two call sites, server limits, and test. No data rollback is needed.

### Verification

- Byte-count, malformed JSON, empty-body, and stalled-body fixtures: passed.
- Full Field suite and production build: passed.

## DA-004 — Canonical workspace containment and sensitive-file hiding

Status: implemented and verified on the current Windows host; platform matrix work remains.

### Objective

Make every Field file, Git-target, terminal-directory, sizing, and attribution decision use
canonical workspace roots while hiding secret material and ignored files from City.

### Blast radius

- New shared workspace-path/visibility module and mount-time canonicalization.
- File tree/read/write, Git target, terminal cwd, adaptive sizing, and harness attribution.
- No source workspace content or Git metadata is modified by the migration itself.

### Security assessment

- Rejects absolute/traversal paths, symlink and Windows junction traversal, canonical escapes,
  non-regular files, missing directory parents, and hidden targets.
- Uses no-follow file descriptors where the platform exposes them and verifies descriptor
  type before reads/writes.
- Hides root `.gitignore` matches, `.env*`, Git/Field state, cloud/SSH stores, package-manager
  credentials, common private-key formats, and configurable deny patterns.
- Historical residual (closed 2026-09-05): nested `.gitignore` files were not yet folded into the visibility policy;
  platform-specific reparse tags beyond Node's symlink/junction classification need hardware
  matrix validation; TOCTOU resistance is strongest where `O_NOFOLLOW` is available.

### Usability and compatibility assessment

- Normal files and creation beneath real existing directories remain unchanged.
- Field intentionally no longer browses or edits ignored files, credentials, private keys,
  `.env*`, symlinked trees, or Git internals. Trusted maintenance of those files moves to an
  external editor/terminal.
- Canonical roots can change only the displayed spelling/case of a mounted path, not its ID.

### Rollback

Revert the shared resolver and its call sites. No data migration is needed; files created by
normal Field edits remain ordinary workspace files.

### Verification

- Canonical root, traversal, sensitive-name, `.gitignore`, regular read/write, new-file parent,
  and symlink/Windows-junction fixtures: passed.
- Full Field suite and production build: passed.
- Update 2026-09-05: nested `.gitignore` semantics pass the maintained-parser regression suite. Nonstandard reparse-point and non-Windows platform qualification remain pending.

## DA-009 — Wire MissionState into the live Knossos loop

Status: implemented and verified across the released Knossos suite.

### Objective

Make the provider-independent mission record follow real work instead of remaining an
unused persistence primitive.

### Blast radius

- Talos task start/resume, compaction, approval, consequential dispatch, verification, and
  final handoff lifecycle.
- Serve state output now exposes non-secret mission identity, phase, revision, and unresolved
  action count.
- Existing provider request/response types and tool behavior are unchanged.

### Security and integrity assessment

- A hash-only action intent is synced before every permitted consequential dispatch and paired
  with a result afterward. A crash between them replays as explicit uncertain work.
- Mission storage never copies raw tool input or output; it retains input/result hashes and
  changed paths. A regression canary verifies source content is absent from the journal.
- Every successful consequential call advances workspace revision, including commands that do
  not report a changed path, and invalidates stale proof.
- Events are limited to 1 MiB, state to 8 MiB, and a journal to 64 MiB. Created files/directories
  use owner-only modes on Unix; Windows continues to rely on the workspace ACL.
- Persistence errors fail the turn instead of silently performing unrecorded follow-up work.
  An already-completed action may still exist when its result cannot be written; its durable
  intent makes that uncertainty visible during recovery.

### Usability and compatibility assessment

- Existing CLI, ACP, serve, provider, dry-run, permission, and plan behavior remains compatible.
- Completed work reaches `handoff` only with current conclusive Oracle proof. Dry-run or
  syntax-only success remains in `verify`, truthfully marked partial.
- Follow-up instructions move a handed-off/paused/blocked mission back through a guarded
  revision path while preserving lineage.

### Rollback

Remove Talos mission hooks and state reporting; retained `.knossos/missions` data is inert and
does not alter workspace source.

### Verification

- Intent/result crash recovery, command-without-path revision, proof invalidation, replay,
  tamper, snapshot, lifecycle, and no-raw-tool-data fixtures: passed.
- Full released Knossos suite: passed (552 tests total).

## FIELD-DATA-001 — Version and verify the operational event store

Status: implemented and verified.

### Objective

Make replayed Field state fail closed on committed corruption while retaining narrow,
crash-safe recovery for a single torn JSONL tail write.

### Blast radius

- `daedalus/field/server/src/store/db.js`: startup integrity/version gates and durable JSONL writes.
- `daedalus/field/server/test/eventlog.test.mjs`: backend health and corruption recovery coverage.
- No event shape, projection rule, API contract, or workspace data changed.

### Security and integrity assessment

- SQLite now runs `quick_check`, refuses unknown future schema versions, and records schema 1.
- JSONL now validates contiguous sequence numbers and record shape. Malformed committed history
  stops startup instead of being silently omitted; only one malformed final record is recoverable.
- Redaction and payload bounds still run before either backend persists an event.

### Usability and compatibility assessment

- Valid schema-0 SQLite databases migrate in place to schema 1 without rewriting events.
- A genuinely torn final JSONL write remains automatically recoverable.
- JSONL now fsyncs each append. This trades fallback-backend throughput for crash durability;
  high-throughput installations continue to use SQLite by default.

### Rollback

Revert the store and test changes. SQLite's `user_version = 1` is metadata-only, but an older
binary would need to tolerate that version marker.

### Verification

- Event-log reopen, health, redaction, torn-tail recovery, and committed-corruption tests: passed.
- Full Field replay and 100,004-event stress suites remain part of the release gate.

## KNS-DATA-001 — Separate production and rehearsal state

Status: implemented and verified.

### Objective

Let the Roman Field demonstrate a full synthetic agent operation without allowing rehearsal
activity to become production truth.

### Blast radius

- Event-store schema 2 adds the canonical event source (`observed`, `derived`, `manual`, or
  `synthetic`) while preserving existing event data.
- The Field projection routes synthetic events into a resettable rehearsal partition.
- The World UI deliberately swaps to that partition only while a rehearsal is active and shows
  a persistent synthetic-state watermark.
- Campaigns, normal traces, production sessions, files, assignments, costs, and workspace growth
  continue to read the production partition.

### Security and integrity assessment

- The source is assigned at the event-log boundary; callers cannot create an unrecognized source.
- Legacy events migrate to `observed`, except legacy simulation records whose existing simulated
  marker deterministically maps to `synthetic` in JSONL.
- Starting a rehearsal resets only synthetic projection state. It cannot clear or mutate production
  state, invoke providers, request permissions, execute commands, or write workspace files.

### Usability and compatibility assessment

- The rehearsal still animates the production event schema and World renderer, but its sessions,
  growth, assignments, graph, and totals are shown as one isolated demo lens.
- Ending the run returns immediately to production state; starting another run deterministically
  clears the prior synthetic projection.
- Existing SQLite stores migrate transactionally from schema 0/1 to schema 2 by adding a defaulted
  source column. Event consumers receive one additive `source` field.

### Rollback

Revert the projection/UI source partition and event-store migration together. A downgraded binary
must tolerate SQLite `user_version = 2` and the additive source column.

### Verification

- Event-log tests verify persisted source classification.
- World tests prove synthetic sessions, files, and workspace growth do not enter production state,
  the UI lens selects only rehearsal state, and a second rehearsal resets only that partition.
- Production Vite build: passed (1,829 modules).

## KNS-DATA-002 — Make Roman Field maturity evidence-derived

Status: implemented and verified.

### Objective

Prevent visual city growth or verification percentages from being manufactured by a session
ending, a verifier role label, discovered folders, file counts, or fallback constants.

### Blast radius

- Campaign objective completion now requires at least one evidence entry per declared
  definition-of-done criterion and persists the criterion-to-evidence mapping.
- Workspace snapshots carry server-derived activity, completion, verification, persistence,
  reliability, score, tier, and bounded evidence references.
- The Roman Field consumes that authoritative metric object and exposes recent supporting
  events in the maturity inspector.

### Security and integrity assessment

- Completion requires explicit criterion evidence.
- Verification requires both completed criteria and the latest independent referee verdict to
  be `verified` while the campaign is in a verified/promoted phase.
- Persistence requires verification plus a checkpoint bound to a content-shaped Git/SHA-256
  revision. Arbitrary checkpoint labels do not count.
- Activity is based only on recent observed tool/filesystem events and is displayed separately;
  it contributes nothing to maturity score or tier.

### Usability and compatibility assessment

- Existing one-criterion objectives remain source-compatible with one evidence entry. Objectives
  with several completion criteria must now submit several evidence entries; this intentional
  tightening can reject previously vague completion reports.
- Legacy satisfied-objective events derive criterion mappings positionally when enough historical
  evidence exists. Missing historical proof remains honestly incomplete.
- Infrastructure activity remains visible but no longer presents itself as verified growth.

### Rollback

Revert the director, projections, metric UI, and tests as one slice. Event additions are backward
compatible; older readers ignore criterion mappings and workspace maturity fields.

### Verification

- Tests prove done sessions produce zero maturity, completion alone does not verify, a verdict
  alone does not persist, a valid revision checkpoint closes the chain, and evidence IDs are
  inspectable.
- Director tests prove incomplete criterion coverage fails closed.

## KNS-DATA-003 — Converge assignment lifecycle state

Status: implemented and verified.

### Objective

Ensure an assignment reaches one honest terminal state when its admitted sessions complete,
fail, are cancelled/reassigned, or disappear across a Field restart.

### Blast radius

- The live Registry tracks admitted assignment members and their terminal outcomes.
- Assignment creation excludes missing or workspace-incompatible sessions and returns a reason
  for every skipped member.
- Projection state now represents `completed`, `failed`, `cancelled`, and `interrupted` with a
  terminal timestamp and reason.
- Startup derives an interruption event for an active assignment whose processes are all gone.

### Security and integrity assessment

- Settlement is emitted exactly once per assignment; duplicate process-close events cannot
  manufacture repeated transitions.
- Reassignment terminates the old assignment instead of leaving two apparent owners for one
  session.
- Restart recovery records a derived event in the durable log rather than silently mutating the
  projection.

### Usability and compatibility assessment

- Successful `session.turn_complete` or clean exit completes an assignment after every admitted
  member reports success. One error or cancellation terminates it immediately and names the
  originating session.
- An assignment with no eligible live sessions is created and immediately cancelled with an
  actionable reason, so callers retain an ID and complete audit trail.
- New terminal states are additive; clients that already render unknown states conservatively
  remain compatible.

### Rollback

Revert Registry settlement, projection handlers, and startup reconciliation. Existing terminal
events remain safe for older projections to ignore.

### Verification

- Registry tests cover multi-member success, duplicate completion, and failure settlement.
- Syntax checks and the existing cancel, replay, restart, and director suites remain release gates.

## CAM-SEC-001B — Remove child-process secret arguments

Status: implemented and verified.

### Objective

Prevent local process inspection, command/status serialization, dry-run output,
or debug formatting from exposing Mesh credentials, callback keys, request
bodies, and managed `llama-server` API keys.

### Blast radius

- Added one daemon-local `curl` adapter used by Cameo Link and hub callbacks.
- Extended `CommandSpec` with a non-serialized secret environment channel.
- Changed managed `llama-server` authentication from `--api-key <secret>` to
  its officially supported `LLAMA_API_KEY` environment input.
- Added one test-only `serde_json` dependency to the placement crate.

### Security and integrity assessment

- Outbound URL, bearer header, and JSON are quoted into curl's standard-input
  config. The process argument list is fixed to `curl --config -`.
- Newlines, quotes, backslashes, and supported controls are escaped; unsupported
  controls and invalid UTF-8 request bodies fail closed, preventing config-line
  injection.
- Secret child environment values are omitted from serialization and redacted
  in both display and debug output. They are still available to the launched
  process and to the same OS account/root; OS keyring-backed delivery remains a
  hardening opportunity.
- Existing HTTPS-only outbound policy, TLS 1.2 minimum, timeouts, and HTTP status
  checks are unchanged.

### Usability and compatibility assessment

- No HTTP wire format or operator workflow changed.
- Current llama.cpp supports `LLAMA_API_KEY`; older locally supplied server
  binaries that predate that environment input must be upgraded.
- Dry-run commands now show `LLAMA_API_KEY='<redacted>'`, intentionally requiring
  the operator to supply the secret before manually executing the preview.

### Rollback

Restore direct curl arguments and `--api-key`; no stored data migration is
involved. This rollback would intentionally reopen the process-list disclosure.

### Verification

- Curl argument-exclusion and config-injection tests: passed.
- Command argv, serialization, display, and debug redaction tests: passed.
- Full Cameo workspace tests: passed (254 tests total).

## CAM-SCP-001B — Product vocabulary and claim synchronization

Status: implemented and verified.

### Objective

Give the multi-device product one consistent, testable vocabulary and keep
public claims aligned with the machine-readable capability manifest.

### Blast radius

- Standardized **Cameo Mesh** as the request-level pool and **Cameo Link** as the
  outbound node agent across README, website, architecture, API, and harness docs.
- Added a dedicated secure pairing and dispatch guide.
- Added a cross-surface test tied to the canonical capability manifest.
- Narrowed the website title and added hardware-compatibility caveats; the visual
  structure and rendered asset choices are unchanged.

### Security and integrity assessment

- Public surfaces now state that mTLS and distributed sharding are unavailable,
  that legacy farm-token dispatch is opt-in, and that Mesh does not pool VRAM.
- Pairing docs describe state sensitivity, revocation ordering, HTTPS callbacks,
  owner-only files, and the current Windows ACL/keychain qualification gap.
- CI fails if core public surfaces lose the Mesh/Link names or contradict the
  canonical negative capability assertions.

### Usability and compatibility assessment

- Existing `/hub/*` routes and legacy `cameo fleet` CLI remain compatible.
- Users now have one end-to-end pairing example, one strict dispatch example,
  explicit retry semantics, and a direct capability-discovery path.
- “Any AMD” is retained as product direction in the README but is qualified as
  non-certification; the website headline is narrowed from “any” to “your.”

### Rollback

Revert documentation/site wording and remove the cross-surface test. There is no
runtime or stored-state migration.

### Verification

- Capability manifest type/truth and public-surface synchronization tests: passed.
- `cargo fmt --check`, full workspace tests, and all-target checks: passed.

## DA-007B — Field release asset pruning

Status: implemented and verified.

### Objective

Keep the Roman Field and Atlas visuals intact without shipping unused source
variants in the production web payload.

### Blast radius

- Moved three unreferenced capital PNG iterations from `web/public` to the
  existing `design/living-rome` source archive.
- The runtime continues to use the optimized `capital-tier-3.webp`; the Atlas
  relief and all four Roman module WebPs are unchanged.

### Security and integrity assessment

- No executable code, route, browser permission, CSP rule, or operator data was
  changed.
- Source search confirmed no runtime reference to the moved PNGs, and the Vite
  build copied the still-referenced assets successfully.

### Usability and compatibility assessment

- Visual output is unchanged.
- The production directory fell from 12,821,338 to 5,566,354 bytes, removing
  7,254,984 bytes (56.6%) from first-install/update transfer and disk use.
- Full-resolution iterations remain available to designers outside the runtime
  public directory.

### Rollback

Move the three PNG variants back into `web/public/assets/living-rome`; no code or
state migration is required.

### Verification

- All 25 Field suites passed, including security, campaigns, simulation, world,
  WebSocket burst/reconnect, and 100,004-event stress replay.
- Production Vite build passed (1,829 modules); full and production-only npm
  audits both reported zero known vulnerabilities.

## CAM-MESH-001A — Add policy-aware mesh admission and deterministic routing

Status: implemented and verified as a preview scheduling slice; secure device pairing, mTLS,
durable reservations, discovery, and failover execution remain open.

### Objective

Turn fleet dispatch from a warm-model/VRAM heuristic into an explainable admission decision that
also honors trust posture, health, protocol compatibility, privacy locality, owner reclaim,
session affinity, deadlines, and latency/throughput preference.

### Blast radius

- New pure mesh scheduler in `cameo-placement`; the older router remains available unchanged.
- `/hub/dispatch` accepts additive scheduling hints and returns additive decision evidence.
- Existing request bodies continue to route under protocol major 1 with legacy farm-token nodes
  allowed by default.

### Security and integrity assessment

- Untrusted, draining/offline, owner-reclaimed, protocol-incompatible, non-fitting, and
  deadline-ineligible nodes are hard exclusions rather than score penalties.
- `local_only` fails closed for farm dispatch because the current hub cannot prove same-device
  locality. No request may opt a network node into being considered local.
- Current enrollment is assigned `legacy_token` by the hub, never trusted from node telemetry.
  Callers can require pairing now (`allow_legacy_token:false`) and receive no route until paired
  enrollment actually ships.
- Model, quant, task, tier, port, priority, expected-output, deadline, protocol, and affinity
  inputs are bounded and validated before roster processing.
- This slice does **not** create a secure mesh transport. HTTPS is required for callbacks, but
  shared farm-token enrollment and bearer callback keys remain the active trust mechanism.

### Usability and compatibility assessment

- Selection is deterministic, including node-id tie-breaking, and every success includes trust,
  health, protocol, affinity, warm-state, predicted completion, and a human-readable reason.
- Eligible affinity is retained; owner reclaim and every hard policy still override it.
- Unknown performance data uses conservative finite defaults instead of making nodes disappear.
- A local-only request currently returns an explicit no-eligible-node response rather than
  silently leaking work to another device.

### Rollback

Switch `cameod::dispatch` back to the original `route` call and remove the additive fields. The
new placement module is pure and stores no state, so rollback requires no migration.

### Verification

- Three-node heterogeneous completion estimate, affinity, owner reclaim, local-only, trust,
  protocol, health, deadline, legacy strict mode, malformed bounds, warm routing, and stable-tie
  fixtures: passed (13 targeted scheduler/dispatch tests).
- Cameo full-workspace and real multi-device qualification: pending.

## CAM-SCP-001A — Establish a canonical typed capability manifest

Status: implemented and verified for daemon discovery; CLI, console, site, ISO metadata, and CI
claim generation remain open.

### Objective

Give machines and people one versioned document that separates stable, preview, planned, and
unsupported behavior without turning roadmap intent into a runtime claim.

### Blast radius

- Added `contracts/cameo-capabilities-v1.json` as the checked-in source of truth and a typed
  `cameo-api` representation.
- Added authenticated `GET /api/capabilities` discovery and embedded the same manifest in
  `cameo-engine/v1` under the additive `product_capabilities` field.
- Existing engine-discovery fields remain byte-shape compatible for older Knossos clients.

### Security and integrity assessment

- The manifest explicitly reports shared-token mesh enrollment, absent device pairing, absent
  mTLS, and absent cross-node sharding. It cannot honestly be read as a secure-pairing claim.
- Capability discovery uses the same consumer-or-operator authentication boundary as engine
  discovery and remains `no-store`.
- Typed deserialization fails tests/build integration if required claims are missing or malformed.
  The manifest contains no addresses, keys, inventory, or other node-specific secrets.

### Usability and compatibility assessment

- Clients can distinguish unavailable roadmap work from an old server or temporary failure.
- The existing `capabilities` block is preserved; `product_capabilities` is additive.
- Automatic recommendation and one-action setup are deliberately reported unavailable until
  their executable workflows and verification exist.

### Rollback

Remove the endpoint, additive engine field, `cameo-api` dependency, typed structures, and manifest
file. Existing discovery consumers continue to use the unchanged v1 fields.

### Verification

- Typed manifest parse/truth assertions: passed (4 `cameo-api` tests total).
- Consumer discovery authorization and engine backward-compatibility fixtures: passed (6
  targeted daemon app tests).
- Cross-surface generation and public-claim CI: pending.

## CAM-HW-001A / CAM-UX-001A — Hardware-aware recommendation and one-action local setup

Status: implemented and verified as a preview CLI workflow; calibrated hardware certification,
runtime installation, GUI first boot, transactional rollback, and offline bundle selection remain
open.

### Objective

Let a user ask Cameo what should run on the detected machine, or use one command to detect,
recommend, download, verify, configure, and start a useful local endpoint for chat, coding, or
Knossos agent work.

### Blast radius

- Added a pure workload/capacity recommender to `cameo-models` and serde support for its contract.
- Added `cameo recommend` and `cameo setup`; existing run, serve, pull, and model commands are
  unchanged.
- Updated the canonical capability manifest from planned/unavailable to preview/available for
  these exact CLI surfaces.

### Security and integrity assessment

- Recommendations are restricted to existing curated aliases with pinned SHA-256 digests; an
  unpinned profile cannot be selected.
- Setup evaluates hardware placement, non-zero port, remote-bind authentication, and runtime
  availability before downloading. A non-loopback endpoint still requires an API key.
- Model acquisition reuses Cameo's HTTPS-only, disk-space-preflighted, partial-file-isolated,
  SHA-256-verified pull path. The server starts only after successful verification.
- `--dry-run` performs no network, cache, or process mutation. `--download-only` never opens a
  listener.
- Runtime installation is not attempted implicitly. The command fails with guidance when the
  packaged `llama-server` is absent, avoiding surprise package-manager or privilege changes.

### Usability and compatibility assessment

- Recommendations explain workload, fit class, context, estimated runtime footprint, and reason.
- A full accelerator fit is preferred over host offload; unknown/insufficient capacity falls back
  to the smallest pinned profile instead of inventing support.
- The default endpoint is loopback. JSON and human-readable previews expose the exact model,
  placement, command, download need, and whether the server would start.
- The recommender currently ranks only three pinned catalog models and uses analytical footprint
  estimates, not measured tokens/second or per-card qualification evidence. It is labeled preview.

### Rollback

Remove the two CLI variants and the pure recommendation types/functions, then mark both manifest
claims planned/unavailable again. Setup stores only an ordinary verified model-cache entry; it can
be managed with the existing `cameo model` commands.

### Verification

- Accelerator-fit, accelerator-vs-host preference, CPU host-offload, smallest-fallback, and pinned
  catalogue fixtures: passed (`cameo-models`, 30 tests total).
- CLI compile and command tests: passed (8 tests).
- Mixed APU+dGPU `recommend --workload agent` and mutation-free `setup --dry-run` fixture:
  passed; selected `llama3.2-3b`, ROCm, 32,768-token configured context, loopback endpoint.
- Real model download/runtime launch, throughput calibration, and hardware matrix: pending.

## CAM-ENG-001A — Atomic mesh admission leases and request idempotency

Status: implemented and verified in-process; durable restart recovery and distributed lease
consensus remain open.

### Objective

Prevent simultaneous hub dispatches from each observing the same free VRAM, and prevent a client
retry from starting the same requested placement twice.

### Blast radius

- `AppState` now owns a bounded admission book used by `/hub/dispatch` execute and advisory calls.
- Execute requests accept an additive `request_id` and return admission identity, state, reserved
  bytes, expiry, and replay status.
- Node supervision remains the final authority; the hub reservation covers only its network
  decision-to-heartbeat consistency window.

### Security and integrity assessment

- Selection and reservation occur under one lock; active/committed reservations are included in
  subsequent node capacity, closing the hub-side time-of-check/time-of-use gap.
- At most 4,096 admissions are retained and each expires after 45 seconds. Arithmetic saturates.
- Reusing a `request_id` with a different body is rejected. Matching pending, committed, and failed
  retries never repeat the remote start action.
- A failed node push immediately releases capacity while retaining failed idempotency state. A
  successful push stays reserved briefly until heartbeat telemetry can reflect the endpoint.
- Admission IDs are correlation identifiers, not authentication capabilities; `/hub/dispatch`
  remains operator-authenticated.

### Usability and compatibility assessment

- Legacy callers may omit `request_id`; they receive a generated correlation ID but do not gain
  cross-request idempotency. New harness clients should always send one.
- Replays return explicit pending/committed/failed semantics. Committed replays include the same
  endpoint without starting another server.
- Reservations are currently process-memory state. Hub restart loses them, so the feature is not
  yet described as durable.

### Rollback

Restore direct dispatch selection and remove `AppState.admissions` plus additive response fields.
Leases expire in memory and create no disk migration.

### Verification

- Identical retry, conflicting retry, concurrent overbooking, failed-capacity release, and legacy
  routing fixtures: passed (10 targeted dispatch tests).

## CAM-SEC-001A — Harden configured credential ingestion

Status: implemented and verified for daemon startup and key documents; OS keychain integration and
Windows ACL certification remain open.

### Objective

Reject weak, malformed, duplicated, oversized, symlinked, or broadly readable credentials before
the daemon exposes inference or control-plane routes.

### Blast radius

- Console, farm, and serve secrets now pass one shared validation policy at startup.
- JSON key documents are size/type/permission checked and reject duplicate credentials or invalid
  labels.
- Existing well-formed 16–4,096 byte credentials and role behavior are unchanged.

### Security and integrity assessment

- Secrets reject leading/trailing whitespace and control characters, preventing header ambiguity
  and accidental newline injection.
- Duplicate values across roles fail closed instead of depending on entry order.
- Key files must be regular non-symlink files under 1 MiB; Unix files with any group/other access
  are rejected with `chmod 600` guidance.
- Validation errors identify only the credential class, never the secret value.
- Sixteen bytes is a compatibility floor, not proof of entropy. Generated production keys and a
  platform keychain remain required release work.

### Usability and compatibility assessment

- Misconfiguration now fails at startup with a direct remediation instead of surfacing as an
  unexplained authorization problem later.
- Existing short development secrets will need rotation. This is an intentional release-security
  break, limited to unsafe configuration rather than an API wire change.

### Rollback

Remove startup validation and restore direct key-file parsing. No stored credential is rewritten.

### Verification

- Length, whitespace/control, duplicate-role, label-bound, and role authorization tests: passed
  (9 targeted auth tests).

## CAM-MESH-001B — One-time device pairing and per-node identity

Status: implemented and verified as a preview; mTLS certificate binding, automatic discovery, and
multi-device physical qualification remain open.

### Objective

Replace a fleet-wide enrollment secret with an operator-approved, single-use join followed by a
revocable per-node identity, while preserving a named legacy migration path.

### Blast radius

- Added operator `POST /hub/pairings`, code-redemption `POST /hub/pair`, and paired heartbeat
  authentication alongside existing legacy farm-token enrollment.
- The node agent accepts a one-time pairing code, atomically saves the issued credential, and
  reconnects with that credential after restart.
- Farm rows now carry hub-authoritative `paired` or `legacy_token` trust into scheduling and the
  operator roster. Deleting a row revokes its paired credential.
- Added `getrandom` and `sha2` as narrowly scoped cryptographic dependencies.

### Security and integrity assessment

- Pairing codes and device credentials are independent 256-bit OS-random values. The hub stores
  only SHA-256 digests and uses a full digest comparison without data-dependent early exit.
- Codes expire after ten minutes, are single-use, share one generic invalid/expired error, and are
  bounded to 64 pending offers.
- Registration validates the HTTPS callback, node identity bound, and callback operator key before
  consuming the code. The plaintext device credential is returned once in a no-store response.
- A legacy farm-token holder cannot update, redirect, or downgrade an existing paired row.
- Node credentials are written via create-new temporary file, sync, and atomic rename; Unix files
  are mode 0600 and newly created directories 0700. Existing parent-directory permissions are
  never modified. Windows currently relies on the account/workspace ACL and needs explicit ACL
  qualification.
- Paired identity proves possession of a device-specific bearer over HTTPS. It is **not mTLS** and
  is reported separately from transport certificate authentication.

### Usability and compatibility assessment

- A hub can be pairing-only: omitting the farm token closes legacy registration while keeping
  operator-created pairing available.
- The node refuses to overwrite an existing identity; reconnect uses the saved file, while a lost
  or revoked identity asks for deliberate re-pairing.
- Paired hub identity is atomically persisted under the configurable Cameo state directory. Reloaded
  nodes begin offline and become dispatchable only after a credential-authenticated heartbeat.
- The state file stores callback operator keys in plaintext under owner-only permissions because the
  hub needs them for pushes. OS keychain/sealed-secret storage and explicit Windows ACL certification
  remain open.
- Legacy farm-token nodes remain compatible and visibly labeled; strict dispatch can exclude them.

### Rollback

Disable pairing routes and node pairing flags, remove stored node credential files intentionally,
and continue with legacy farm-token enrollment. Removing a paired farm row immediately revokes the
in-memory credential.

### Verification

- Randomness shape, digest match, single-use, expiry-equivalent error, paired authentication,
  anti-downgrade, callback retention, API redemption/heartbeat, and atomic file round-trip tests:
  passed (pairing/hub/app/agent targeted suites).
- Hub restart/offline restore, plaintext-device-secret canary, reconnect, durable revocation, and
  reload-after-revocation fixture: passed.

## DA-010 — Add server-aware per-agent context budgets

Status: implemented and verified across CLI, serve, ACP, Cameo-engine preparation, and the full
released Knossos suite.

### Objective

Keep long-running agents useful without allowing prompt growth or reply reservations to exceed
the context actually exposed by a provider or local serving runtime.

### Blast radius

- New pure context-policy module and an additive `Engine::prepare` hook.
- Cameo, resilience, and quota wrappers forward preparation.
- Talos resolves the effective budget before each turn; CLI flags, serve commands, ACP session
  controls, trace events, and state output expose the same values.

### Security and integrity assessment

- A known engine window is an upper bound; explicit user values cannot raise it.
- Unknown windows use a finite fallback. Completion and protocol reserves are subtracted before
  compaction, with saturating arithmetic for tiny or hostile values.
- ACP rejects zero, negative/non-integer, and out-of-range values and refuses changes while the
  session lock is busy.
- `--no-compaction` remains an explicit ablation and is not silently re-enabled by turn-time
  discovery.

### Usability and compatibility assessment

- Cameo can make a local model resident before Knossos sizes the first request, avoiding a
  misleading pre-load context estimate.
- `--context-window` and `--compact-at` provide operator control; serve and ACP clients can
  inspect or change limits between turns.
- The request completion ceiling is clamped to reserved output room, reducing truncated answers
  and provider context-limit failures.
- Current compaction is structurally safe but still extractive text elision, not the planned
  semantic context compiler. Artifact rehydration and invariant manifests remain open.

### Rollback

Remove the context module, `prepare` hook, front-end controls, and trace fields. No data migration
is required; new trace fields are additive.

### Verification

- Policy arithmetic, hostile clamp, small-window, ACP mutation, serve state, and compaction
  regression fixtures: passed.
- Full released Knossos suite: passed (552 tests total).

## KNS-POL-003 / KNS-POL-004 — Enforce session, routine, campaign, and delegation ceilings

Status: materially implemented and verified. Update 2026-09-05: durable reservations, missing/zero cost distinction, monotonic replay accounting and registry global/endpoint/campaign admission are implemented. Verification and reinforcement inherit source campaign policy and reinforcement joins assignment settlement. Unknown final bill reconciliation, provider-side ceilings and durable fair queues remain open.

### Objective

Stop unattended or concurrent work at enforceable resource boundaries instead of relying on prompt
language, and keep campaign staffing within its declared capacity.

### Blast radius

- Field sessions now enforce cumulative output-token, wall-clock, and dollar limits at the registry.
- Routines pass their own dollar ceiling into the same session admission path.
- Campaign spend is accumulated from per-session deltas, campaign exhaustion pauses the cohort, and
  campaign concurrency is enforced during mobilization, reinforcement, and assignment.
- Unmanaged Claude delegation tools are denied so child count and depth remain under Knossos control.

### Security and integrity assessment

- One durable `budget.exhausted` event is emitted per exhausted session or campaign; repeated usage
  messages cannot create a stop storm.
- Lifetime usage accounting avoids counting streamed and final-result usage twice.
- Campaign totals no longer add cumulative per-session values repeatedly.
- Update 2026-09-05: missing telemetry retains a conservative durable reservation; explicit reported zero is distinguished from missing telemetry. Provider-enforced final charge ceilings and operator bill reconciliation remain open.

### Usability and compatibility assessment

- Existing sessions keep the configured defaults: 32,768 output tokens and 120 wall minutes.
- Routines may narrow the dollar limit without changing interactive session behavior.
- Managed Knossos delegation remains available; only provider-side delegation that bypasses Field's
  policy and accounting is denied.

### Rollback

Remove registry budget metadata/timers and restore direct cumulative campaign accounting. New events
are additive and require no storage rollback.

### Verification

- Usage deduplication, dollar stop, endpoint resilience, assignment settlement, campaign exhaustion,
  concurrency, and delegation-policy fixtures: passed.
- Full Field suite including 100,004-event stress replay: passed.

## KNS-RTN-001 — Make routine execution durable, bounded, and deterministic

Status: implemented for UTC cron and `skip` overlap. Update 2026-09-05: next-run/owner/history display, durable minute claims, interrupted-run failure and calendar/restart tests are complete. Additional timezones, overlap policies, cooldowns and queued-run controls remain open.

### Objective

Make unattended Field work survive restart, obey its own resource policy, and avoid duplicate,
overlapping, or self-triggered execution.

### Blast radius

- Routine enablement now starts from replayed projection state rather than the Git default alone.
- Schedule evaluation is UTC, validates five-field syntax/ranges, follows standard day-field OR
  semantics, and deduplicates each UTC minute.
- Manual, schedule, and filesystem runs share one controller and one registry spawn path.
- The operator UI confirms enablement and shows durable last-run/outcome state.

### Security and integrity assessment

- Enabling unattended work requires an explicit confirmation because a trigger may invoke model,
  filesystem, terminal, or network capabilities.
- Disabled routines cancel pending debounce work; active-run locks prevent overlap; filesystem writes
  from the active run cannot retrigger the same routine.
- Run IDs are random, replay timestamps come from events, synthetic filesystem events are ignored,
  and all scheduler timers are released on shutdown.
- Routine configuration fails closed for unknown roles, endpoints, workspaces, triggers, schedules,
  completion policies, debounce windows, and non-positive dollar limits.

### Usability and compatibility assessment

- “Run now” remains a direct explicit action and also works while a routine is disabled.
- Existing YAML using the documented five-field subset remains compatible; invalid schedules now fail
  at startup instead of appearing enabled but inert.
- Enabling gains one deliberate confirmation. Disabling remains immediate.

### Rollback

Restore direct API spawning and config-default enablement. Routine history events are additive and can
remain in the audit log without affecting older projections.

### Verification

- UTC, day-field semantics, malformed cron, replayed enablement, confirmation, disable-during-debounce,
  overlap, budget propagation, self-trigger prevention, completion release, and timer cleanup: passed.
- Full Field suite and Vite production build (1,829 modules): passed.

## KNS-POL-001 / KNS-POL-002 / KNS-TRUST-001 — Make role and destination policy executable

Status: materially implemented and verified for Field-managed Claude and Knossos sessions;
redirect/DNS-rebinding enforcement, semantic shell parsing, and third-party MCP trust manifests remain
open.

### Objective

Replace role prose and approval assumptions with one server-side policy that constrains actual tool
availability, write paths, environment mode, and network destinations across both harness adapters.

### Blast radius

- Claude now receives `--tools` as an availability boundary and no longer receives
  `--allowedTools`, which had silently auto-approved consequential actions.
- Claude and Knossos tool names map into the same role categories at the permission boundary.
- Per-agent tool loadouts narrow the effective role for both adapters; Architect and Archivist write
  scopes are enforced against workspace-relative paths.
- Web fetches are restricted to declared Field domains and shell-based network clients are denied.

### Security and integrity assessment

- Undeclared tools, unknown consequential tools, workspace escapes, sensitive paths, symlink/junction
  chains, scoped-write violations, and read-only environment mutations fail closed before operator
  approval.
- HTTP(S) destinations reject credentials, loopback, private/link-local ranges, cloud metadata, and
  undeclared domains. Subdomains are accepted only beneath an explicitly declared parent.
- Network-capable shell commands are denied because a shell approval cannot reliably enforce redirects
  or the final destination. Declared network access must use the structured fetch tool.
- The current URL gate cannot attest the final address after remote redirects or DNS changes; a broker
  that resolves and revalidates every hop remains required before this is labeled complete SSRF defense.

### Usability and compatibility assessment

- Read/search/list operations remain available according to each role. Writes, commands, and fetches
  now consistently reach the existing operator permission flow.
- Roles that depended on undeclared built-ins must add them explicitly to version-controlled role
  metadata; this is an intentional least-authority compatibility break.
- New files are admitted when their existing parent is safe. Creating a new nested directory and file
  in one tool call is conservatively denied until a race-safe parent-creation transaction exists.

### Rollback

Remove the `--tools` argument and the category/scope/egress checks. No persisted state or wire schema
depends on these controls.

### Verification

- Claude availability arguments, Knossos/Claude category parity, per-agent narrowing, write scopes,
  missing-path fail-closed behavior, private/metadata/credential URL denial, domain allowlisting, and
  shell-network denial: passed.
- Full Field suite including campaign, prompt-injection, adapter handshake, and stress fixtures: passed.

## KNS-DATA-004 — Bind campaign promotion to truthful Git evidence

Status: record-only mode implemented and verified; managed worktree restore remains open by design.

### Objective

Prevent a campaign from claiming persistence or rollback when Field has neither captured a content
revision nor changed the workspace.

### Blast radius

- Campaign checkpoint requests now derive the target workspace and clean `HEAD` revision on the
  server; caller-supplied revisions cannot manufacture persistence.
- Promotion requires a Git/SHA-256-shaped content revision in both the transition model and director.
- The UI names the operations “Record Git checkpoint” and “Record rollback”; rollback events and
  responses state `mode: record_only` and `workspaceChanged: false`.
- Git observation now runs with a scrubbed child environment, hooks/fsmonitor disabled, credential
  prompts disabled, and external diff drivers disabled.

### Security and integrity assessment

- Dirty, untracked, non-Git, unmounted, and revisionless workspaces fail closed. Field never snapshots
  hidden secrets into an implicit object and never destroys an operator's dirty tree.
- Checkpoint confirmation is explicit and the server ignores any claimed revision in the request.
- Git subprocesses receive no provider, CI, package-registry, or unrelated desktop credentials.
- This mode does not restore files. That limitation is represented in API/event/UI state rather than
  hidden behind the word “rollback.”

### Usability and compatibility assessment

- A verified campaign must be committed before it can be promoted. Dirty-file names are summarized in
  the refusal so the operator can resolve the state deliberately.
- Existing tests and direct domain callers may supply a content revision, but the HTTP product path
  always derives it.
- Managed restore remains withheld until a reviewed operation can preserve staging, untracked files,
  symlinks, and repository filters without destructive reset.

### Rollback

Restore the direct campaign action route and revisionless promotion gate. The added checkpoint and
rollback fields are additive to the event schema.

### Verification

- Clean revision binding, dirty/untracked refusal, malformed revision denial, transition gates,
  campaign lifecycle, and director vertical slice: passed.
- Full Field suite and Vite production build (1,829 modules): passed.

## KNS-PERF-001 / ORG-GOV-001 — Inventory and gate Roman Field artwork

Status: runtime/reference/hash/budget enforcement implemented. Update 2026-09-06: the product owner
attested that all six Field images were generated with ChatGPT/Codex built-in image generation for this repository; they are licensed Apache-2.0
and `npm run release:audit` now passes. The original assessment below is historical except for the
hash/budget gate, which remains binding.

### Objective

Ensure every map asset is intentional, referenced, content-addressed, size-bounded, and legally
reviewed before Knossos ships it.

### Blast radius

- The previously orphaned capital artwork now appears only when the selected capital reaches
  evidence tier III or IV; lower tiers retain the modular settlement growth system.
- Added a canonical JSON inventory, human-readable provenance register, SHA-256 verification,
  reference detection, and a 5.5 MiB asset payload budget.
- Structural asset checks run as part of the normal Field test command. A separate release audit
  fails on unknown creator/source/license/review state.

### Security and integrity assessment

- Hash drift, undeclared files, missing files, dead public assets, and payload growth fail CI.
- The provenance gate deliberately reports all six current assets as `BLOCKED`; no license or creator
  fact was invented from filename or filesystem timestamps.
- Artwork is decorative and receives empty alternative text inside an already labeled settlement,
  avoiding duplicate screen-reader narration.

### Usability and compatibility assessment

- The 359 KiB capital image is not preloaded; it is fetched only after the evidence threshold is
  reached. It was already in the public payload, so the source reference does not increase the shipped
  artifact size.
- The capital becomes a visible reward for verified progress without changing maturity calculations.
- Release maintainers get exact blocked records rather than discovering attribution gaps at publish
  time.

### Rollback

Restore modular capital rendering and remove the manifest/audit scripts. No operational state or
stored event depends on the artwork.

### Verification

- Structural audit: passed (6 referenced files, 5,137,924 bytes, all hashes verified).
- Release audit (2026-09-06): passed after owner attestation filled creator, Apache-2.0 license, and
  APPROVED review status. The gate still fails closed if a future asset is unknown or unverified.

## CAM-SEC-001C — Bound and harden the Cameo HTTP control plane

Status: implemented and verified for the built-in listener; proxy-edge and independent review remain
release operations.

### Objective

Remove request-framing ambiguity, constrain the inference proxy to the API Cameo actually supports,
and prevent brute-force or accidental overload from an individual network client.

### Blast radius

- The HTTP parser now accepts only strict CRLF-framed HTTP/1.0 or HTTP/1.1 requests using
  `GET`, `POST`, or `DELETE`; HTTP/1.1 requires `Host`.
- Malformed request lines and headers, duplicate headers, control characters, bad percent encoding,
  fragments, unsupported methods, and all `Transfer-Encoding` requests are rejected.
- The `/v1` gateway now allowlists only chat completions, completions, and embeddings instead of
  forwarding arbitrary backend paths.
- Every ordinary response carries `nosniff`, frame denial, and no-referrer headers. Bounded
  per-client fixed-window admission now covers public, control, inference, invalid-credential, and
  pairing traffic; discovery publishes the exact limits.

### Security and integrity assessment

- Rejecting transfer encoding closes the content-length/transfer-encoding ambiguity class in a
  server that intentionally implements only content-length bodies.
- Invalid credentials are capped at 30 attempts per network address per minute and pairing at 10;
  the limiter is bounded to 4,096 active identity/class records and fails closed when full.
- Cameo deliberately ignores `X-Forwarded-For`, so an untrusted client cannot choose its rate-limit
  identity. A reverse proxy sees one shared address unless it also performs client-aware admission.
- Limits are process-local, reset on restart, and are not coordinated across Cameo Mesh nodes. They
  are defense in depth, not a substitute for TLS, a firewall, or edge rate limiting.
- Streaming inference relays framing from a supervised loopback backend and therefore does not pass
  through Cameo's ordinary response-header builder. That local backend remains an explicit trust
  boundary; the dashboard is not served through the inference route.

### Usability and compatibility assessment

- Standards-compliant clients using content length are unchanged. Chunked request bodies and
  undeclared `/v1` extensions now fail explicitly instead of being interpreted inconsistently.
- A throttled client receives `429` with `Retry-After: 60`; harnesses can discover all allowances in
  `GET /api/engines` and the capability manifest advertises request admission support.
- The deliberately open loopback development posture receives the normal inference/control budgets,
  not the invalid-credential budget. Local Unix-socket harness control is exempt from network limits.

### Rollback

Remove the request-admission call and restore the broad proxy matcher/parser. Capability and engine
descriptor additions are additive; older consumers ignore them.

### Verification

- Parser/framing/header-injection, route allowlist, authentication, per-client isolation, reset, and
  `Retry-After` tests: passed.
- Workspace formatting and strict Clippy: passed.
- Full Cameo workspace tests: 258 passed, 0 failed.
- Full release workspace build: passed.

## KNS-REP-001 — Import Field into the Knossos release repository

Status: implemented and locally verified; publication remains blocked by the explicit artwork
provenance gate and by clean-tag release policy.

### Objective

Make Roman Field a first-class Knossos product component with one owner, one CI graph, and a
downloadable artifact instead of leaving it as an untracked Cameo-side development tree.

### Blast radius

- Moved the complete Field project to `Knossos-Harness/field/`; local event databases,
  dependencies, and generated bundles remain ignored.
- The default map now mounts the Knossos repository and its Field workspace. Cameo remains an
  inference endpoint, matching the product boundary rather than appearing as Field's owner.
- Knossos CI now installs, tests, and builds Field on Node 22.
- Tagged releases define a deterministic `knossos-field-<version>.zip` containing the server,
  production UI, default policy/configuration, source/tests, license, changelog, asset register, and
  checksum. The publish job waits for both CLI and Field artifacts.

### Security and integrity assessment

- The release job runs the full Field suite and redistribution audit before packaging. The six
  unresolved artwork records therefore fail closed and currently prevent a tag from publishing.
- Archive paths come from an explicit allowlist, are sorted, receive fixed timestamps and modes, and
  reject symbolic-link inputs. Generated state, dependencies, design references, logs, and secrets
  are excluded.
- No Git history, commits, branches, remotes, or existing dirty changes were rewritten.

### Usability and compatibility assessment

- Development commands are unchanged after `cd field`; the bundle can install locked production
  dependencies and start with `npm start`.
- Field retains its own semantic version and changelog while sharing Knossos repository governance.
- A Windows npm workspace junction caused the directory move to leave two locked generated folders
  at the former local path. Product files were recovered into the destination and verified; those
  ignored generated remnants were not deleted or included in either repository.

### Rollback

Move `field/` back out of the Knossos checkout and remove the Field CI/release jobs. No runtime state
schema or product protocol depends on its repository location.

### Verification

- Full Field suite from `Knossos-Harness/field`: passed, including the 100,004-event stress replay.
- Production Field build from the imported location: passed (1,829 modules).
- Both GitHub workflow files parse as YAML.
- Deterministic archive generated with the intended server, UI, configuration, license, source/test,
  asset, and checksum inputs.

## AUDIT-2026-09-05 — Production gap audit and verified closures

Status: audit and listed fixes complete; the full production roadmap remains open.

- Audited all 61 suggested issue IDs against the roadmap, ledger, source surfaces,
  release workflows and local test results. See docs/production-audit.md for the
  issue-level disposition and exact remaining gates.
- Closed the DA-002 maintained Markdown parser/sanitizer residual and DA-004 nested
  ignore residual, with adversarial and Windows junction regression coverage.
- Added strict bounded buffered upstream response validation and socket truncation
  coverage; consumer proxy errors no longer expose private endpoint/OS details.
- Fixed partial ISO publication following failed builds and added the Field OS CI
  matrix. These workflow changes have local YAML validation, not observed CI runs.
- Exposed the canonical capability manifest through CLI, generated checked docs and
  ISO staging. Added lazy City loading to preserve initial JS transfer budgets.
- Validation: Cameo 261 tests, strict Clippy and format pass; Knossos 554 test
  results pass (live-provider qualification remains absent); Field full suite and
  production build pass. Initial JS 91.72 kB gzip; CSS 26.09 kB gzip.
- Six artwork provenance records still block Field release, confirmed by running
  release:audit. Hardware/ISO/update/mTLS/live-provider qualification is open.
- Compatibility: malformed/truncated backend responses now fail with 502; buffered
  responses above 32 MiB fail; ignored/secret files are hidden immediately after
  policy edits; Markdown now follows CommonMark and does not load remote images.
- Rollback: revert only this audit's changes in proxy/app, CLI dependency/dispatch,
  manifest generator/docs/ISO staging, workflow gates, Field Markdown/path modules,
  their tests, package manifests/lock and lazy City import. Preserve all pre-existing
  dirty changes; do not reset whole files or the submodule. No state migration,
  model change, disk operation, publication or deployment occurred.

## CONTINUATION-2026-09-05 ? Recovery, admission and lifecycle

Status: implemented/tested slices; full production roadmap still open.

- Added Cameo durable endpoint intent snapshots with exclusive ownership, bounded
  schema/size/count, checksums, atomic generation commits and previous-generation
  retention. Persisted high-level settings contain no launch command or bearer key.
- Reconcile desired endpoints by replanning, native SHA-256 verification and port
  admission; never trust persisted readiness/PIDs. Bound daemon recovery attempts,
  record observed health/restarts and keep explicit stops stopped across restart.
- Added native bounded-memory model hashing and pinned digest admission. First-use
  custom hashes establish identity, not model license or publisher trust.
- Added allowlisted cameo doctor JSON export with preview and no-overwrite behavior.
- Added expiring browser bootstrap, logout, cookie invalidation, WebSocket revocation
  and stopped reconnect behavior while preserving harness permission capabilities.
- Added durable UTC routine claims, interrupted-run failure, next-run/owner/history
  UI and calendar/restart tests. At-most-once claims can skip a run after a crash.
- Added durable campaign budget reservations, unknown-cost holds, narrowed admission,
  resume denial, monotonic cumulative accounting and global/endpoint/campaign caps.
- Fixed stale child process callbacks clearing a replacement on immediate resume;
  retained draining process ownership for capacity. Tested with real Node children
  for both harness adapters; this does not qualify real cloud/local providers.
- Verification: Cameo 267 tests, strict Clippy and format passed; Field full suite
  including the real process-generation regression and production build passed.
  Capability manifest drift and both repository whitespace checks passed.
  Doctor export and refusal to overwrite passed. No deployment or ISO install.
- Compatibility: daemon startup now requires writable exclusive endpoint state;
  corruption fails closed. Existing endpoints are hashed on start. Default Field
  global cap is 16 and per-endpoint cap 4. UTC/skip is the supported routine subset.
- Remaining: durable leases/adoption/migration/restore and power/AMD fault tests;
  signed transactional updates; mTLS mesh; provider cost ceilings/reconciliation;
  broader scheduler/UI/mission acceptance and existing artwork provenance blockers.
- Rollback requires a deliberate version decision: older Cameo ignores the new
  endpoint intent store and loses its restart guarantees. Back up state with the
  daemon stopped; do not delete it or roll back entire dirty source files.

## KNS-POL-003 / KNS-POL-004 follow-up ? Preserve campaign policy on follow-up work

Status: implemented and locally verified (2026-09-05).

Baseline: verify and reinforce commands omitted campaign identifiers when spawning.
Their children could receive an independent session allocation and bypass the source
campaign's budget/concurrency. Reinforced sessions were also absent from assignment
settlement membership.

Changes: registry admission derives campaign/mission/team/objective/environment from
source session or active assignment before budget/capacity checks. Conflicting campaign
IDs, missing sources, settled/mixed-campaign assignments and unavailable/invalid
campaign budget policies fail closed. Reinforcements join assignment membership.

Verification: actual spawn-path tests (external launch stubbed) reject verification
and reinforcement when source funds are reserved; reject campaign substitution; and
prove admitted reinforcement retains campaign identity and settlement membership.
Full Field suite passed, including process lifecycle, security, budget, campaign, routine, replay and 100,004-event stress tests. Log: target/production-audit/ledger-followup-field.log.

Compatibility: invalid follow-up requests now fail instead of creating detached work.
This does not prove full narrower-policy delegation or provider billing enforcement.
Rollback: revert only follow-up admission/membership and regression changes; no event
migration is required. Preserve prior dirty changes and durable budget history.

## DA-006 / KNS-POL-003 follow-up ? Restart authority and deadlines

Status: implemented and verified (2026-09-05).

Baseline: ended sessions revoked their permission token and cleared their deadline
but resume reused the old MCP token and did not restore the wall-time timer. This
could break permissions while letting resumed work outlive its original deadline.

Changes: both adapters invoke a registry-owned pre-start hook. It rechecks budget
and capacity, rearms the original absolute deadline and rotates direct-harness
permission authority on subsequent starts. New tokens are registered for redaction;
old tokens remain invalid. Resume does not grant a new time budget.

Verification: actual security authority confirms old-token refusal and new-token
acceptance across end/resume; registry fixture verifies deadline restoration and
redaction registration. Full Field suite passes. Native lifecycle fixtures remain
local Node processes, not live provider qualification.

Blast radius: Field registry, both harness adapters and registry regression suite.
No persisted schema or provider protocol change. Rollback would restore the revoked
capability/deadline defects; no event-history rewrite is required. The same-user
process inspection and opaque credential broker residuals remain open.

## CAM-STATE-001 follow-up - Offline endpoint-state check, backup and restore

Status: implemented and locally verified (2026-09-05); full H9 remains partial.

Baseline: snapshot integrity existed inside daemon startup, but no supported offline
operator interface could verify, export or restore the endpoint-intent store.

Changes: cameod state check/backup acquire the same exclusive ownership lock as the
daemon. Backup verifies the current snapshot and refuses existing output files.
Restore validates schema, checksum and bounds before staging a new verified store,
then publishes to a new destination directory. Existing stores are never replaced;
no process is started. Failed staging can leave a hidden diagnostic directory.
Unsupported schema versions fail closed; this is not a schema migration facility.

Verification: locked-store backup refusal, backup/restore round trip, output collision
refusal and corrupt-backup refusal pass. Actual CLI restore/check/backup smoke passes
on a local empty-intent fixture. Cameo 268 tests and strict all-target Clippy pass.
Evidence: target/production-audit/state-tools-rust.log and state-*-cli.log.

Scope/limits: exports contain endpoint settings and model paths, not model weights,
credentials, mesh identities or leases. Full appliance backup, durable leases,
process adoption, GPU reconciliation and power-loss qualification remain open.

Blast radius: cameod endpoint store, daemon CLI and supervisor-recovery runbook.
Rollback: remove the additive commands; existing version-1 daemon state remains
compatible. No existing user state, model or process was changed by validation.

## CAM-STATE-001 follow-up - Durable lease ownership

Status: implemented and locally verified; full H9 remains partial (2026-09-05).

Version-2 endpoint snapshots now persist bounded session lease ownership alongside
endpoint intent. Version-1 snapshots verify with their existing checksum and migrate
on the next commit. Unsupported schemas and malformed lease identities fail closed.

Claims and releases commit before changing in-memory ownership or reporting success.
Storage failure returns 503; a failed release retains its record. Session deletion
and stale-session cleanup now handle persistence failures instead of claiming success.
Recovered claims report recovery_required until explicitly reclaimed against a live
model. They cannot make a dead endpoint ready. Backup/restore includes lease ownership.

Verification: durable reopen, unavailable model refusal, failed-release rollback,
successful release across restart and v1-to-v2 migration tests pass. Full Cameo suite
and strict Clippy pass; logs are target/production-audit/durable-leases-*.log.

Update 2026-09-05: recovered-claim expiry and durable active heartbeat renewal now pass (see follow-up below). Remaining: session-board recovery, verified process adoption,
GPU allocation reconciliation and full power-loss/appliance acceptance. Recovered
claims conservatively protect endpoints and can require explicit operator release.
This is local ownership durability, not distributed lease consensus.

Compatibility: v2 state is not readable by the older v1-only daemon; preserve backups
before any binary downgrade. No live user leases or models were changed by testing.

## CAM-STATE-001 follow-up - Bounded orphan lease lifetime

Status: implemented and locally verified (2026-09-05).

Baseline: persisted recovered leases could protect an endpoint indefinitely when
its session never reconnected. Cleanup of ordinary stale sessions depended on
control API traffic and could miss owners absent from the in-memory session board.

Changes: claims carry a persisted 90-second deadline; active session heartbeats
renew it through the same transactional store. Recovered claims require explicit
reclaim; a heartbeat cannot resurrect expired ownership. The maintenance loop
expires claims independently of dashboard/API requests. Failed expiry writes keep
ownership and retry, while lease status reports expired rather than active.

Recovery keeps existing deadlines, bounds future timestamps after clock rollback,
and assigns legacy records one persisted recovery window. A monotonic runtime
limit prevents backward wall-clock steps from holding a live claim indefinitely.

Verification: orphan expiry across reopen, restart non-extension, future-clock
clamping, failed-expiry persistence, recovered heartbeat non-reclaim and late
heartbeat non-resurrection tests pass. Full Cameo suite: 272 tests pass. Strict
Clippy passed before the final added heartbeat regression; format checks pass.
Logs: target/production-audit/lease-expiry-*.log.

Compatibility: ownership now needs heartbeats within 90 seconds, matching the
existing live-session policy. No model is stopped by lease expiry; it only releases
eviction protection. Full process/GPU reconciliation and session-board recovery
remain open. Rollback requires preserving the v2 state and understanding that an
older binary will not enforce lease deadlines; do not rewrite production history.

## CAM-STATE-001 follow-up - Session identity recovery

Status: implemented and locally verified (2026-09-05); complete appliance recovery remains open.

Baseline: endpoint/lease ownership survived restart but the session board vanished.
Heartbeat clients could also supply their own resident VRAM capability record.

Changes: version-3 snapshots retain bounded session identity (id/name/role/mode/
engine/model), excluding mission text, paths, changed files and proof claims.
Daemon startup reconstructs records as recovery_required with no live heartbeat or
resource authority. A fresh successful heartbeat is required before resource claims.
Session identity persists before board updates; deletion atomically removes both
identity and lease. Client-supplied VRAM authority is ignored; only supervisor
operations can change it. Version-1/2 snapshots remain readable and upgrade on commit.

Verification: failed-write non-activation, recovered identity/stale status, heartbeat
non-forgery, private mission/path canaries and durable deletion pass. Full Cameo
suite: 274 tests pass; strict Clippy and format pass. Evidence:
target/production-audit/session-recovery-rust.log and session-recovery-clippy.log.

Limits: external agent execution and full mission history are not resumed by this
board; harness journals own that state. Process adoption/GPU reconciliation and
power-loss acceptance remain open. Source snapshots are not complete appliance
backups. Downgrading to an older state reader requires an explicit migration plan.

## CAM-STATE-001 follow-up - Confirm owned-process exit before releasing capacity

Status: implemented and locally verified (2026-09-05).

Baseline: stop and eviction ignored kill/wait errors, dropped child handles and
could admit replacements without observing exit. A failed try_wait also discarded
ownership, making an unknown process look absent to admission.

Changes: termination first checks for exit, requests termination, then polls with a
one-second deadline. Errors/timeouts retain the child handle and disable endpoint
routing/restart. A kill/exit race succeeds only after exit is observed. Refresh wait
errors also retain ownership. Eviction commits desired stops, marks all victims
non-restarting, confirms exits, then commits the replacement intent before spawning.
A failure cannot silently treat unreleased process capacity as free or persist a
replacement start prematurely. Earlier successfully evicted endpoints stay stopped
if a later victim fails; the API reports failure for operator intervention.

Verification: injected kill failure, timeout and exit race cases pass; a real
owned test child is terminated and reaped. Full Cameo suite: 277 test results pass
(including the child fixture entry); strict Clippy passed before the final small
eviction-state adjustment. Logs: target/production-audit/process-stop-*.log.

Compatibility: unsuccessful stop/eviction returns an error and leaves a visible
failed endpoint with its owned PID. Operators retry stop after correcting the cause.
No unrelated processes are adopted or killed. Live GPU-memory reconciliation,
OS orphan inventory, hardware faults and planned graceful drain remain unfinished.
Rollback would restore false-success process accounting; preserve existing intent
snapshots and dirty source when reviewing changes.

## CAM-STATE-001 / CAM-API-001 follow-up - Gateway drain admission and lifetime

Status: implemented and locally verified gateway slice; planned drain remains partial.

Baseline: no operator drain admission gate or response-lifetime count existed.
Changes: atomic gateway admission, RAII request permits retained through HTTP/SSE
response delivery, operator POST/GET/DELETE drain controls, retryable 503 for new
inference, health availability and non-ready probes while draining. Retried drain
requests cannot extend the existing deadline. Status is explicitly gateway-scoped.

Verification: active permit lifetime, dropped response, stream write failure,
deadline non-extension, operator authorization, retryable inference rejection and
health/readiness behavior pass. Full Cameo suite: 281 test results pass; strict
Clippy passed before the final status-label clarification. Logs: drain-*.log under
target/production-audit. An initial test expected 403 for a consumer's operator
request; the existing auth contract correctly returns 401, and the assertion was fixed.

Remaining: deadline-triggered cancellation, runtime-direct traffic, mesh/control
work admission, persistent update integration and OS shutdown handling. gateway_idle
is not a claim that all device work has stopped. No system service was drained or
stopped in this development run. Rollback removes additive drain controls/accounting;
no persisted schema changes or user-state edits are involved.

## CAM-API-001 / CAM-STATE-001 follow-up - Drain deadline cancellation

Status: implemented and locally verified for connected gateway upstreams (2026-09-05).

Requests now register owned upstream sockets with their admission generation.
Deadline polling shuts those sockets down. Cancellation remains sticky for old
generations after operator resume, while new admission uses a fresh generation.
Buffered cancellation returns retryable 503; streams before any relayed bytes return
503, and started streams fail without fabricated DONE or a second HTTP response.
Socket registrations and admission permits release when response handling ends.

Verification: real silent TCP fixtures cover buffered and streaming cancellation;
a midstream fixture proves explicit failure without fabricated success. The first
Windows test showed shutdown alone did not wake the read reliably, so bounded
250ms read polling was added while preserving the 300-second idle timeout. New
connections have a five-second TCP connect timeout. Full Cameo suite: 283 test
results pass; strict Clippy and format pass. Logs: drain-cancel-*.log.

Remaining: OS resolver deadlines, blocked downstream delivery, direct runtime/mesh
traffic and update/shutdown orchestration. Socket cancellation is tested, but a real
pinned llama-server must still prove generation cancellation and KV/GPU release.
No production daemon was drained or stopped. Rollback removes the additive gateway
cancellation wiring; no persisted schema or user data changes were made.

## CAM-API-001 / CAM-STATE-001 follow-up - Downstream cancellation and bounded DNS

Status: implemented and locally verified for TCP (2026-09-05).

Baseline: a non-reading client could hold response delivery after upstream drain;
OS DNS resolution had no caller deadline. Changes register downstream sockets with
the same cancellation generation and wrap response writes with terminal cancellation
checks. 250ms write polling preserves the existing 30-second idle timeout while
allowing timely drain cancellation. ConnectionAborted avoids write_all retrying an
Interrupted error indefinitely. Unix-stream wiring is included but unexecuted here.

DNS now has a two-second caller limit and four outstanding workers. An abandoned
OS resolver retains its capacity slot until completion; hung DNS cannot accumulate
unbounded threads. Numeric hosts bypass DNS; TCP connect remains bounded at five
seconds. A hard drain may close the client connection without a final JSON error.

Verification: a real non-reading TCP client test and delayed resolver fixture pass.
The first targeted downstream test passed, but the full suite exposed unreliable
Windows shutdown wake-up; write polling fixed it. Full suite and confirmation run:
285 test results pass. Strict Clippy passes. Evidence: drain-io-*.log.

Remaining: Unix execution, runtime-direct/mesh traffic, actual backend KV release,
OS shutdown/update integration and physical fault acceptance. No production service
was changed. No persistent-state format or user files changed in this slice.

## CAM-STATE-001 follow-up - Planned daemon/service stop

Status: implemented and locally verified state transitions; Linux service qualification open.

Changes: pinned ctrlc termination handler, stoppable TCP accept loop, sealed gateway
drain, new-start refusal and automatic-restart suppression. Stop waits up to 30s
for gateway completion, cancels at deadline, allows 5s completion grace, then stops
owned endpoints within a 15s aggregate window without removing desired intent.
Failures exit nonzero. Systemd now uses KillMode=mixed and TimeoutStopSec=75 so its
first signal does not prematurely kill children before the daemon can drain.

Verification: shutdown cannot reopen admission; intent survives stop/reopen;
new model starts are refused; the listener stops without incoming traffic and
serves a real TCP request before stopping. Accepted sockets explicitly restore
blocking mode for portable parsing. Full Cameo suite: 289 test results pass;
strict Clippy and format pass. Logs: target/production-audit/shutdown-*.log.

Remaining: actual OS signal/systemd execution, direct-runtime/mesh traffic,
startup interruption and real runtime/GPU release. Terminal group Ctrl-C may
reach children before drain, so service SIGTERM is the intended production path.
No user daemon or service was stopped during this run. Rollback reverts signal/
listener/service changes and dependency additions; preserve durable endpoint intent.

## Coordinated completion - Roadmap ownership and release build gates

Status: plan established; CI configuration verified locally; remote execution open.

User requested explicit Sol, Terra and Luna orchestration. Their lanes are Cameo,
Knossos runtime and Field respectively; coordinator owns integration, workflows
and shared evidence. docs/completion-plan.md defines dependencies, acceptance and
review protocol; docs/completion-issues.md accounts for all 61 unique roadmap
issues without declaring unverified requirements complete. Detailed lane plans
record production paths and remaining acceptance work.

CI changes require locked Cameo builds/tests, release-mode builds for both Rust
products, and a Knossos Linux/Windows/macOS Rust matrix. Tagged Knossos packaging
now first runs format, strict Clippy and tests on the exact release source. CI
workflows use read-only repository permissions unless explicitly overridden.

Inspection also found an ISO invocation defect: one docker exec environment
option appeared after the container name, making Docker try to execute -e instead
of bash. Both Cargo target options now precede the container and command.

Verification: four changed workflows parse as YAML without duplicate keys; the
ISO build command's option ordering was checked; all 61 issue IDs are unique and
assigned. No GitHub job, Docker build, ISO boot or release publication was run.
These changes add executable gates, not passing qualification evidence. Existing
asset provenance failures remain binding. Rollback removes these additive CI
checks or restores the specific workflow hunk; no persisted product data changed.

## CAM-ENG-001 follow-up - Strict discovery before provisioning (2026-09-06)

Status: implemented and locally verified. Knossos previously treated a present
non-string contract_version as an unversioned legacy descriptor and accepted
incomplete v1 fields. The Cameo adapter now rejects malformed versions/routes,
invalid/duplicate model IDs, invalid identity/auth/state, profiles and limits,
and a node without advertised chat completions before attempting model startup.
Truly unversioned descriptors remain supported; malformed v1 cannot downgrade.

Verification: nine Cameo adapter tests pass, including a real local HTTP fixture
whose malformed discovery cannot fall through to model provisioning. Full nested
Knossos all-target suite passes: 561 test results (two optional live tests can
early-return and are NOT live qualification). Strict Clippy passes. Logs are
target/production-audit/engine-contract-{tests,clippy,full}.log. No actual model,
cloud provider or user daemon was started. Complete cross-provider negotiation,
feature ceilings and black-box pinned-backend acceptance remain open.

Rollback restores permissive adapter parsing only if the compatibility tradeoff
is intentionally accepted; this change has no persisted schema migration.

## KNS-RTN-001 follow-up - Queue-one recovery and cooldown (2026-09-06)

Status: implemented for queue-one and UTC; broader scheduler gate remains open.
The delegated first version discarded queued work after restart and could strand
an idle-cooldown queue. Review also found a released queue slot while its timer
was pending, and cooldown reconstruction from start instead of completion.

The coordinated change has one durable queued record, a single dispatch pump,
claim-before-spawn, and budget re-admission at dispatch. The coordinator added
restoration of unclaimed work, completion and cooldown in the same event, bounded
queued paths, persist-before-memory enqueue, stopped-controller guards, and a
SHA-256 binding to routine/role/agent/endpoint/workspace/default configuration.
Configuration changes cancel old queued authority; already claimed work becomes
interrupted on restart rather than blindly repeating a consequential operation.
Older queued records without this binding are explicitly skipped. Stop preserves
unclaimed work; explicit disable cancels it. No migration rewrites old events.

Verification: existing routine suite and new routine-queue-recovery.test.mjs pass
cooldown-only enqueue, timer overlap, replay, configuration change, disable,
failed persistence, failed budget admission and claim interruption scenarios.
Full Field suite and production build pass after the queue recovery changes;
final live-configuration guard also passes the repeated full Field suite. Evidence:
target/production-audit/queue-recovery-{field,build}.log. Initial JS is 92.49 kB
gzip; CSS is 26.09 kB gzip. Windows process-launch permission was required for
the existing child-process/esbuild checks. No external routine was executed.

Remaining: parallel/replace overlap, richer timezone/DST behavior, multi-mission
fairness and external-action reconciliation. Rollback must disable queue-one
configurations before returning to a version supporting only skip; retain event
history and never replay a claimed run automatically.

## Coordinated review - Environment and update boundaries (2026-09-06)

Knossos environment records now use bounded streaming SHA-256 hashes of project
instructions, manifests and lockfiles, compare host facts, and reject drift at
verification. Historical journal inspection remains available when records are
legacy or drifted. IMPORTANT: open_for_resume is a guarded store helper; no
production CLI/serve/ACP path restores a persisted Talos conversation yet. A
portable checkpoint, original policy/budget restoration and uncertain-action
reconciliation remain LOOP-004/006 implementation gates. Do not count helper
restart tests as completed cross-process agent continuation.

Cameo's rolling updater was replaced by signed offline verification/preflight.
Apply/commit/rollback are disabled for the existing single-root installer.
Filesystem A/B fixtures and a Linux wrapper-signature test were added, but the
latest simulator remains under review for lock ownership, exact input binding,
slot non-overlap, cleanup safety and durable promotion. It is not a host update
engine or a bootable rollback implementation. Linux signature/boot tests have not
run here. CI now includes update tests and shell lint; the ISO explicitly includes
OpenSSL. docs/updating.md and build comments match the actual behavior and no
longer imply rolling-package or re-flash rollback safety.

All three delegated agents stopped on an account usage-limit error. The
coordinator continued local review and queue corrections; no usage reset was
redeemed. Their remaining assignments and the full 61-issue scope remain open.

## CAM-UPD-001 follow-up - Bound A/B transaction engine (2026-09-06)

Status: local simulator accepted for fixture use; host apply remains disabled.
The coordinator closed the review findings against `scripts/update_ab_simulator.py`:
every mutation re-parses the hashed layout/manifest bytes under the exclusive
writer lock; `begin` refuses to clobber an existing journal; nested and
persistent path aliases still fail closed; interrupted promotion after
`os.replace` and before journal commit is discarded on recover so retry can
stage the same generation. BIOS firmware is accepted at this layout layer only.

Verification: `python -m unittest tests.test_update_ab_simulator tests.test_update_bundle`
passed 19 checks with one skipped Linux wrapper-signature test on Windows.
This is not bootloader, power-loss, ESP, or installed-host evidence. Real
`cameo-update apply` still exits closed on the single-root installer.

Rollback restores the previous simulator and tests. No appliance state is
migrated.

## CAM-API-001 / CAM-ENG-001 follow-up - Negotiated feature and limit enforcement (2026-09-06)

Status: implemented and locally verified. The `/v1` gateway now rejects native
tools, logprobs, logit_bias, n!=1, multimodal image parts, json_schema
response_format, and best_of/echo before model routing. Served context windows
cap `max_tokens`. The Knossos Cameo adapter no longer claims native tools by
default, applies discovery `tool_calls.native`, `max_completion_tokens`,
`max_request_bytes`, and context bounds, and refuses to put tools on the wire
when native calls are not advertised.

Verification: Cameo `openai` unit tests and `gateway_rejects_unadvertised_openai_features_before_routing`
pass; workspace `cargo test --locked` and `cargo clippy --workspace --all-targets --locked -- -D warnings`
pass. Nested Knossos `cameo::tests` pass, including discovery feature application.
Live pinned llama-server and cloud conformance remain missing evidence.

Rollback restores permissive proxy forwarding and the previous Cameo adapter
capability defaults. No persisted schema change.

## CAM-SCP-001 follow-up - Site and console claims from the typed manifest (2026-09-06)

Status: implemented for generated surfaces. `scripts/render-capabilities.mjs`
now writes `docs/capabilities.md`, `site/capabilities.json`, and the marked
table in `site/index.html`. CI `--check` covers all three. The console fetches
`GET /api/capabilities` into a Capabilities panel. Certification rows still
have no attached release evidence.

Verification: `node scripts/render-capabilities.mjs --check` passes after
generation. Dashboard HTML is compile-included; no browser QA was run.

## KNS-RTN-001 follow-up - Replace and parallel overlap (2026-09-06)

Status: implemented for skip, queue-one, replace, and parallel. Replace
interrupts the active session through `registry.command('cancel')` and starts
the new run. Parallel admits concurrent runs and keeps `activeRunIds` through
projection replay. UTC-only schedules and cooldown as a start-rate limit remain.
Timezone/DST expansion and multi-mission fairness remain open.

Verification: `server/test/routines.test.mjs` and `routine-queue-recovery.test.mjs`
pass. Full Field scripted suite passes after the overlap changes, including the
100,004-event stress fixture. Production Vite build was not re-run for this
slice; `npm run release:audit` remains a failing artwork-rights gate.

## KNS-LOOP-004 correction - CLI conversation restore (2026-09-06)

The 2026-09-06 environment review overstated the resume gap. `knossos task
--persist-conversation` and `--resume-mission` restore a portable checkpoint
into Talos, recheck environment drift, and refuse MCP/delegation/dry-run
restore. Serve and ACP still do not expose that path. Cross-process CLI
evidence lives in `tests/conversation_recovery.rs`. This does not close
LOOP-004/006.
