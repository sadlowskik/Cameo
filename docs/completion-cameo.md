# Cameo production completion lane

This document tracks implementation work for Workstream H without treating local
fixtures as release or hardware certification. The canonical requirements remain
in `PRODUCTIZATION_PLAN.md`; the implementation ledger and production audit remain
the source of recorded evidence.

## Dependency order

1. **H12 transactional updates** builds on H9's version-3 endpoint/session state,
   authenticated drain API, and bounded service shutdown. Establish a signed,
   immutable bundle; fail-closed preflight; bootable rollback point; staged apply;
   reboot health commit; and offline rollback before updating any component.
2. **H9/H8 recovery and serving completion** proves owned-process adoption,
   hardware reconciliation, cancellation resource release, feature negotiation,
   admission fairness, and black-box compatibility against the pinned runtime.
3. **H10/H11 artifact and storage transactions** add a content-addressed model
   catalogue, license/template policy, quarantine, signed offline model bundles,
   encryption, quotas, granular reset, and whole-appliance backup/restore.
4. **H13 mesh security and failure handling** adds discovery, mutual TLS identity,
   signed expiring capacity advertisements, portable Cameo Link, remote stream
   cancellation, owner reclaim, and partition recovery.
5. **H1/H14 product truth and operations** make the capability manifest the source
   for every surface and add bounded logs, metrics, retention, alerts, clock-state
   reporting, and a restorable support bundle.
6. **H4/H5/H6/H7/H15/H16 qualification** runs ISO, hardware, UX, Knossos, thermal,
   soak, failure, update, and mesh matrices. Only retained results from exact
   release artifacts can promote a combination to `certified`.

## Acceptance gates H1-H16

Current evidence locations used to assess these gates are: H1
`contracts/cameo-capabilities-v1.json`, `cli/src/main.rs`, and
`scripts/render-capabilities.mjs`; H2/H8 `cameod/src/auth.rs`, `app.rs`,
`proxy.rs`, `rate_limit.rs`, and `resolve.rs`; H3/H10 `core/models/src/lib.rs`;
H4/H15 `core/gpu-detect/` and `scripts/phase1/`; H5/H11 `archiso/` and its
installer/storage scripts; H6 `cli/src/main.rs` and `cli/src/doctor.rs`; H7
`contracts/cameo-engine-v1.schema.json` and the Knossos adapter; H9
`cameod/src/endpoint_store.rs`, `sessions.rs`, `supervisor.rs`, and `drain.rs`;
H12 `archiso/airootfs/usr/local/bin/cameo-update`, its helpers under
`usr/local/lib/cameo/`, and `scripts/update_ab_simulator.py`; H13
`cameod/src/pairing.rs`, `dispatch.rs`, and `core/placement/src/mesh.rs`; H14
the daemon health/metrics surfaces and `cli/src/doctor.rs`. H16 has no retained
real-hardware certification evidence in this checkout.

| Gate | Acceptance evidence still required |
|---|---|
| H1 | Typed manifest now generates README pointer, docs, site table/JSON, console panel, CLI, ISO, and engine claims; CI rejects docs/site drift; certification entries still lack exact release evidence. |
| H2 | Mock backend/node credential propagation, bind and socket-mode matrix, bounded control-plane limits, identity-bound callbacks, DNS/internal-address abuse tests, and independent review. |
| H3 | Disk-preflighted resumable acquisition, cancellation cleanup, quarantine and atomic promotion, full provenance/license metadata, real inference readiness probe, and typed lifecycle failures. |
| H4 | Versioned records from representative real AMD systems covering boots, runtime/model combinations, memory pressure, suspend/driver failure, performance, and thermals. |
| H5 | Reproducible signed full/lite ISO build, UEFI/BIOS boot, offline install, persistence, update/rollback, export, and granular reset on installed systems. |
| H6 | Three explainable workload profiles and advanced override; journaled runtime/model installation; representative inference/tool probe; atomic promotion/rollback; offline and accessibility proof. |
| H7 | Independently upgradeable pinned Knossos bundle, scoped token and Unix socket launch, plus restart/eviction/refusal/SSE/reboot session recovery tests. |
| H8 | Published feature matrix, strict capability rejection, normalized responses, disconnect cancellation with resource release, bounded fairness, and black-box tests against Cameo, pinned llama-server, and Knossos. |
| H9 | Crash-safe reconciliation through process, daemon, power, GPU, OOM, disk, database, model and port failures; verified ownership/adoption; observed capacity; migrations and whole-state restore. |
| H10 | Content-addressed immutable records, bounded GGUF inspection, license/auth/template policy, runtime evidence invalidation, lease-aware undoable GC, signed import/export, and derived-artifact lineage. |
| H11 | Stable disk identity and destructive recheck, exact storage plan, encryption, safe resume, separated state/models, quotas and reserve, granular reset, replacement-hardware restore, and media/fault matrix. |
| H12 | Signed full-component manifests and offline bundles, compatibility preflight, bootable atomic rollback, drain/state snapshot/apply/reboot/health commit, migration downgrade safety, channels with evidence, and interruption tests at every boundary. |
| H13 | Mutual enrollment and revocation, signed fresh snapshots, one endpoint across three heterogeneous nodes, admission/affinity/cancel/reclaim, policy-bound transfer, and partition/mixed-version/failure matrix. |
| H14 | Stable structured events and bounded metrics, retention/rotation/privacy levels, actionable alerts, clock-jump ordering, and previewed redacted archive with verified restore. |
| H15 | Calibrated component memory model, allocation probes, prediction error bounds, safe power profiles, thermal protection, and repeatable hardware-bound performance/quality evidence. |
| H16 | Every ten-item appliance scenario and the additional mesh matrix pass on each certified combination, including a 24-hour soak. Fixtures cannot certify hardware. |

## Current H12 implementation boundary

The first transaction increment replaces the rolling package update path with an
offline signed-bundle verification protocol. It verifies the manifest signature
and every component digest, rejects symlinks, traversal, duplicate destinations
and unsafe modes, and checks disk headroom without mutating the system. `apply`
fails closed because the current installer creates a single ext4 root and has no
bootloader-coordinated A/B layout. A Btrfs root snapshot alone is insufficient:
bootloader root flags may ignore a changed default subvolume, a read-only snapshot
is not a writable boot root, and the separate boot/EFI contents can diverge from
the package database.

The apply implementation starts only after the installer owns an explicit A/B
layout contract. Before mutation it must persist and sync a transaction journal
containing release ID, active/inactive slots, boot generation, state-backup digest,
and phase. The state machine is `verified -> preflighted -> drained -> state_saved
-> inactive_written -> boot_trial -> health_committed`, with rollback from every
phase. The daemon's version-3 endpoint backup is captured after drain. All OS,
kernel, firmware, GPU stack, runtimes, Cameo, Knossos, schemas and starter-model
identities belong to one signed manifest. The bootloader grants a bounded trial to
the inactive slot and restores the prior slot automatically unless local hardware,
model and API probes commit it. Models and mutable state live outside both slots.

This increment does not apply updates and does not complete H12. H12 remains open
until the A/B layout and state machine exist and are exercised on installed Linux at
every interruption boundary, release signing keys are managed by the publication
pipeline, schema migrations declare downgrade compatibility, and all supported
prior-version upgrades preserve models and state.

### Boot trial and interruption model

The simulator follows systemd's Automatic Boot Assessment contract. A trial entry
named with `+3` is renamed before each attempt to decrement tries-left and increment
tries-done. A successful boot reaches `boot-complete.target` only after Cameo's
hardware, representative inference, and API checks; `systemd-bless-boot` then
removes the counters. After the counter reaches zero, systemd-boot orders that bad
entry behind a good entry and the prior healthy slot wins. Cameo must use the
assessment-aware preferred selection; `LoaderEntryDefault` explicitly ignores boot
assessment and therefore cannot implement this rollback policy. See the official
systemd [Automatic Boot Assessment](https://systemd.io/AUTOMATIC_BOOT_ASSESSMENT/)
and [Boot Loader Interface](https://systemd.io/BOOT_LOADER_INTERFACE/) contracts.

`scripts/update_ab_simulator.py` now models recovery at every journal phase. Before
`boot_trial`, recovery retains the prior healthy slot and discards the inactive
candidate. A trial gets three bounded attempts, success blesses the new slot, and
three failures fall back to the prior slot. The journal binds the manifest, layout,
persistent partition identity, and persistent-state digest; a mismatch fails closed.
Tests cover interruption before every pre-trial boundary, every failed trial,
successful blessing, interruption after commit, slot aliasing, BIOS layouts,
missing persistence, and changed persistent state.

The reusable fixture adapter now validates nonempty slot, device, persistence,
and state identities; canonicalizes both slot roots beneath the adapter root; and
rejects resolved path aliases. It takes an exclusive writer lock, writes journals
through a synced temporary file and atomic rename, and syncs the parent directory
on Unix. Staging is bounded, digest checked, confined to the inactive fixture
slot, and atomically promoted only after all files arrive. Interrupted staging is
removed during recovery. Recovery re-hashes the supplied layout and manifest
against the journal before trusting any recorded phase.

This is deterministic regular-file simulation, not bootloader or power-loss proof.
It does not render/rename actual Boot Loader Specification entries, invoke
`systemd-bless-boot`, or exercise a real ESP, UKI, filesystem, block device,
reboot, or inference runtime. Linux CI executes the real filesystem lock, staging,
torn-write, OpenSSL signature, and interruption fixtures; Windows runs the pure
validation and recovery model because this managed checkout blocks subprocess
creation beneath Python-created test directories.

## Coordinator review, 2026-09-06

The filesystem simulator is accepted as a regular-file transaction fixture, not
as a host update engine. Mutation entry points re-parse bound layout and
manifest bytes under the exclusive writer lock, `begin` cannot clobber an
in-flight journal, nested/persistent aliases fail closed, and a crash after
atomic promotion but before journal commit is discarded on recover. BIOS is
accepted only as a layout label; the fixture still does not render Boot Loader
Specification entries. Windows: 19 unittest checks plus one skipped Linux
wrapper-signature check. Linux CI is configured, not observed. Real host
`apply` remains disabled until the installer owns an A/B layout.

Gateway follow-up: `/v1` now rejects unadvertised OpenAI parameters and caps
`max_tokens` to the served context window. This is local contract enforcement,
not pinned-backend conformance.
