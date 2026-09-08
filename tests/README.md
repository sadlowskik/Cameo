# tests — cross-crate integration tests

Per-crate unit and integration tests live with their crates (for example,
`core/gpu-detect/tests/`). This directory holds cross-component Python fixtures
for the installer and signed A/B update transaction. The Rust workspace suite and
these Python tests are both required by CI.

Run everything with:

```bash
cargo test --workspace
python -m unittest tests.test_build_update_bundle tests.test_update_bundle \
  tests.test_update_ab_simulator tests.test_update_host_transaction \
  tests.test_installer_ab_contract -v
```
