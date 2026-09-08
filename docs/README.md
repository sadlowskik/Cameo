# Cameo documentation

The project has one roadmap: [the Cameo + Knossos productization
plan](../PRODUCTIZATION_PLAN.md). Current evidence is summarized in
[release readiness](release-readiness.md). Historical audits, completion lanes,
implementation ledgers, and superseded product plans are intentionally not kept
as competing sources of truth.

## Start here

- [Quickstart](quickstart.md) — install and reach first local chat.
- [Capabilities](capabilities.md) — generated maturity labels from the checked-in
  capability manifest.
- [Release readiness](release-readiness.md) — what was actually observed on the
  current checkout.
- [Productization plan](../PRODUCTIZATION_PLAN.md) — scope, blockers, workstreams,
  release evidence, and definition of v1.

## Reference

- [Architecture](architecture.md)
- [HTTP API](api-reference.md)
- [Knossos integration](harness-integration.md)
- [Bundled Knossos](../daedalus/README.md) — runtime, Field, and research boundary.
- [Cameo Mesh](cameo-mesh.md)
- [GPU tiers](tiers.md)
- [Inference tuning](inference-tuning.md)
- [Endpoint recovery and support bundles](supervisor-recovery.md)
- [Updates](updating.md)

## Installation and validation

- [Proxmox guest](proxmox.md)
- [Secure Boot](secure-boot.md)
- [Hardware validation runbook](../scripts/phase1/RUNBOOK.md)
- [Tester roll](testers.md)

Component-local operational notes remain beside the component: `archiso/`,
`containers/`, `k8s/`, `tests/`, and `scripts/phase1/` each have their own README
or runbook.
