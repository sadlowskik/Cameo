# Cameo capabilities

Generated from contracts/cameo-capabilities-v1.json. Run `node scripts/render-capabilities.mjs` to update.

These labels describe implemented software surfaces, not hardware certification or production release approval. See [release readiness](release-readiness.md) and the [canonical productization plan](../PRODUCTIZATION_PLAN.md) for outstanding gates.

The same manifest is available from `cameo capabilities`, daemon capability discovery, the console Capabilities panel, `site/capabilities.json`, and `/etc/cameo/capabilities.json` in newly built ISOs.

| Capability | Maturity | Available | Scope |
|---|---|---|---|
| inference.openai_compatible_gateway | stable | yes | Authenticated /v1 gateway for chat completions, completions, and embeddings. |
| inference.streaming | stable | yes | Server-sent event streaming is proxied without response buffering. |
| inference.native_tool_calls | unsupported | no | Tool orchestration is currently owned by the agent harness. |
| harness.knossos_session_control | stable | yes | Authenticated session board and Knossos control routes are available. |
| harness.vram_leases | stable | yes | Session-owned VRAM leases prevent surprise eviction by default. |
| harness.context_discovery | stable | yes | Per-model context limits are exposed in engine profiles. |
| models.verified_downloads | stable | yes | Known aliases are pinned by SHA-256 and incomplete downloads are isolated. |
| models.hardware_recommendation | preview | yes | Detected accelerator and host headroom select the highest-ranked checksum-pinned profile for chat, coding, or agent work. |
| models.one_action_setup | preview | yes | cameo setup preflights policy and runtime, downloads a pinned model, verifies SHA-256, configures serving arguments, and starts on loopback by default. |
| mesh.scheduling | preview | yes | Trust, health, locality, affinity, deadline, warm-model, and load-aware admission is available on /hub/dispatch. |
| mesh.device_pairing | preview | yes | An operator creates a ten-minute one-time code; redemption issues a per-node 256-bit credential stored only as a hub-side digest. Node and hub identity state survives restart; removal durably revokes it. Certificate binding remains pending. |
| mesh.mutual_tls | planned | no | Hub-to-node callbacks require HTTPS but are not yet mutually authenticated with device certificates. |
| mesh.distributed_model_sharding | planned | no | Cameo routes whole inference requests; it does not shard one model execution across LAN nodes. |
| security.role_scoped_keys | stable | yes | Consumer and operator credentials are separated and compared in constant time. |
| security.local_harness_bypass | stable | yes | Keyless harness control is restricted to local transport in self-host posture. |
| security.request_rate_limits | stable | yes | Bounded per-client fixed-window admission protects pairing, invalid credentials, inference, control, and public HTTP routes. |
| security.release_security_review | preview | yes | Security regression tests exist; independent release review and platform certification remain pending. |
