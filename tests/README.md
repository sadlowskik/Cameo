# tests — cross-crate integration tests

Per-crate unit and integration tests live with their crates (e.g.
`core/gpu-detect/tests/`). This directory is for **cross-crate** end-to-end tests
that exercise several components together — added as the surface that spans crates
grows (e.g. CLI ↔ API ↔ core once the Phase 2 socket transport exists).

Run everything with:

```bash
cargo test --workspace
```
