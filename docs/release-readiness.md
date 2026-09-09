# Release readiness

Snapshot: 2026-09-08

Cameo: `311f7cd` / `v0.2.0-beta.3`

Bundled Knossos baseline: `9916bdc` / target `v0.2.0-beta.2` (unpublished)

Next package targets: Cameo `v0.2.0-beta.4`; Knossos and Field
`v0.2.0-beta.2`. These versions are not published yet.

The canonical requirements and execution order are in
[PRODUCTIZATION_PLAN.md](../PRODUCTIZATION_PLAN.md). This page is only a current
evidence snapshot.

## Verdict

Cameo and Knossos are **beta, pre-v1**. Cameo's software core and Knossos's
deterministic suite are locally green, but the current appliance has no retained
real-hardware certification record. Field's current run reached a sandbox-denied
process-spawn test before the remaining suite could execute.

Do not label this checkout stable or release-candidate qualified.

## Evidence observed in this checkout

| Product | Observed result | Interpretation |
|---|---|---|
| Cameo | `cargo test --workspace` passed on Windows | Locally verified software paths; not Linux, ISO, or AMD-hardware proof |
| Knossos | All 572 tests passed, including cross-process conversation recovery | Locally verified deterministic baseline |
| Knossos evaluation | 27 fixtures pass the frozen-suite audit. Windows `knossos-rs` tests passed; Python 1069 passed / 5 skipped. Live two-arm runner against `gemini-3.6-flash` on 2026-09-08 stopped on token-budget/tool-calling failures before 8/8 (`daedalus/reports/reward-hacking/20260908-211431/`) | Grader and suite are locally verified. The live run is retained infrastructure evidence, not a capability score |
| Field | Every directly runnable test passed, including the 100,004-event stress case; process generation and Vite/esbuild both hit sandbox `EPERM` | Broad local evidence; rerun the complete suite/build where child spawn is allowed |
| Hardware | Tester roll is empty; no `known-good-combo.json` is present | No hardware combination is certified from this checkout |
| Release metadata | Next Cargo/npm/display versions are aligned; the site block is generated from `release-manifest.json`; mislabeled legacy downloads were removed | Metadata is consistent; all next-version artifacts remain explicitly unpublished |

## Present release blockers

1. Complete Field's full test/build run on supported systems.
2. Finish a complete two-arm Knossos live matrix without agent/provider abort,
   retain both equal-budget arms, and obtain independent review before publishing
   a claim. The 2026-09-08 Gemini Windows run is not that matrix.
3. Build both current Cameo ISO editions from a clean tag and retain QEMU boot,
   install, reboot, update, rollback, and recovery evidence.
4. Produce the first complete real-AMD record with
   `scripts/phase1/RUNBOOK.md`; calibrate claims from evidence.
5. Publish current Cameo ISO artifacts so the public artifact version catches up
   with the aligned source and package version.
6. Run artifact-only offline Cameo -> Knossos -> Field acceptance.
7. Complete the platform, security, accessibility, supply-chain, fault, soak,
   and dogfood gates in the canonical plan.

## What may be claimed now

- Cameo implements fixture-tested detection, placement, authenticated serving,
  persistent endpoint intent, preview setup/recommendation, and preview mesh
  scheduling/pairing.
- Knossos implements a Rust coding harness with bounded tools, durable missions,
  policy, verification, recovery primitives, ACP/serve/CLI/REPL, and a Cameo
  adapter.
- Field implements a local authenticated operator surface with durable event and
  campaign machinery.

Do not claim universal AMD compatibility, hardware certification, pooled VRAM,
distributed model sharding, mTLS mesh identity, full recovery qualification, or
stable-release readiness.

## Commands for the next snapshot

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

cargo fmt --all --check --manifest-path daedalus/knossos-rs/Cargo.toml
cargo clippy --manifest-path daedalus/knossos-rs/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path daedalus/knossos-rs/Cargo.toml
Push-Location daedalus/model
python scripts/audit_suites.py --lock ../knossos-rs/cases/suite-lock.json
Pop-Location
daedalus\scripts\run-reward-eval.cmd

npm --prefix daedalus/field test
npm --prefix daedalus/field run build
node scripts/render-capabilities.mjs --check
node scripts/render-releases.mjs --check
node scripts/check-doc-links.mjs
```

The ISO, hardware, Secure Boot, update-interruption, mesh, and artifact-only gates
must run in their required environments; a Windows source-tree run cannot replace
them.
