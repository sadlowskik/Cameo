# cameod HTTP API reference

The control-plane daemon serves three surfaces on one port (default `9090`):

- **Operational probes** — unauthenticated, for humans, k8s, and scrapers.
- **`/api/*`** — the console/admin API, gated by the **console key** (bearer)
  when one is configured (`CAMEO_CONSOLE_KEY`).
- **`/v1/*`** — the OpenAI-compatible inference gateway (F8), gated by the
  **serve key** (`serve_api_key`) when one is configured — separate from the
  console key.

Errors are JSON: `{ "error": "…", "status": <code> }`.

## Operational (unauthenticated)

| Method · Path | Returns |
|---|---|
| `GET /healthz` | `{ "status": "ok", "hub": <bool> }` — process is up (F9); `hub` is whether this daemon accepts `/hub/*`. |
| `GET /readyz` | `{ "ready": true }` / `503` — can detect + plan (F9). |
| `GET /version` | `{ "name": "cameod", "version": "…" }` (F5). |
| `GET /` | The dashboard (HTML). |

`GET /metrics` (Prometheus text: daemon/endpoint/GPU gauges, F11) requires the
**console key** when one is configured — the default ISO/container bind is
all-interfaces, and an open `/metrics` there hands any network peer the model
names and GPU inventory. Point Prometheus at it with its standard bearer-token
scrape config:

```yaml
scrape_configs:
  - job_name: cameo
    authorization: { credentials: <console key> }
    static_configs: [{ targets: ["<box>:9090"] }]
```

A keyless daemon (loopback dev) serves `/metrics` openly, like every other route.

## Console API (`/api`, console key)

| Method · Path | Purpose |
|---|---|
| `GET /api/gpus` | Detected GPUs, tiers, links, host RAM, bottleneck. |
| `GET /api/node` | This node's self-description — identity, topology, tiers, endpoints — for the fleet controller / k8s (F13). |
| `GET /api/engines` | Harness discovery: `/v1` base, `auth_required`, served models, and a versioned capability block (F15). |
| `GET /api/models` | Cached models, aliases, cache dir. |
| `POST /api/models/gc` | Remove interrupted `.part` downloads (F12). |
| `DELETE /api/models/{name}` | Remove a cached model (F12). |
| `POST /api/plan` | Preview a placement plan for a model (no spawn). |
| `GET`, `POST /api/sessions` | List or heartbeat a harness session for the Deck. |
| `DELETE /api/sessions/{id}` | Remove a session and release its model lease, if any. |
| `GET`, `POST`, `DELETE /api/sessions/{id}/lease` | Inspect, claim, or release an explicit session-to-model residency lease. |
| `GET /api/servers` | List supervised endpoints (state, VRAM, restarts, uptime). |
| `POST /api/servers` | Plan + start an endpoint. `507` if it won't fit VRAM; `409` when only active session leases prevent eviction (F10). |
| `GET /api/servers/{id}` | One endpoint's live view. |
| `DELETE /api/servers/{id}` | Stop and forget an endpoint. |

`POST /api/servers` / `POST /api/plan` body: `{ "model": "<name|path>", "host"?,
"port"?, "params"?, "quant"?, "moe"?, "context"?, "backend"?: "auto|vulkan|rocm|cpu" }`.

## Inference gateway (`/v1`, serve key)

| Method · Path | Purpose |
|---|---|
| `GET /v1/models` | OpenAI-style list of served models. |
| `POST /v1/chat/completions` | Routed by `model` to the serving `llama-server` and proxied (F8). |
| `POST /v1/completions`, `/v1/embeddings` | Same routing. |

The gateway holds each endpoint's serve key and injects it upstream, so a client
presents **one** key to **one** door for **many** models.

A request body with `"stream": true` is streamed: the upstream `text/event-stream`
(SSE) response is relayed to the client token-by-token as it is produced, rather
than buffered and returned as one blob. Non-streaming requests are buffered as
before.

### Harness engine contract

`GET /api/engines` preserves its original flat fields (`node`,
`openai_base_path`, `auth_required`, `models`, `posture`, `local_harness`) and
also advertises `contract_version: "cameo-engine/v1"`. New harnesses may use
the additive `capabilities` block plus `session_api_path` and
`operator_api_path`; older clients can ignore them safely. `operator_ensure`
means the node supports model lifecycle through the operator API â€” it does
not grant a consumer credential permission to load VRAM.

The descriptor's `engine_state` is `idle` when no model is resident and `ready`
otherwise. `limits.max_request_bytes` is Cameo's hard HTTP body ceiling;
`limits.max_completion_tokens: null` means Cameo does not impose a second
generation cap beyond the selected endpoint and request. `tool_calls.native` is
currently `false`: clients must use their agent-managed fallback rather than
assuming a particular `llama-server` tool-call dialect. Cameo still forwards the
complete OpenAI request body and streams SSE responses verbatim.

`model_profiles` is an additive list for currently running models. It carries
the endpoint id, model, backend, requested `context_tokens`, and `lease_count`.
`vram_bytes` is included only for an operator-authorized discovery request (or
the local self-host socket); it is deliberately absent from consumer discovery.

### Session leases

`POST /api/sessions/{id}/lease` accepts `{ "model": "<running model>" }` and
returns a stable endpoint claim. It never starts a model. While a claim is
active, normal LRU admission cannot evict that endpoint; a new model request
that would require it receives `409`, never an implicit eviction. `DELETE`ing
the lease or its session makes the endpoint evictable again.

A harness must refresh its session with `POST /api/sessions` at least every 90
seconds while it needs the claim. Stale sessions remain visible in the Deck for
diagnosis but their leases are released on the next Cameo request. If an operator
stops or the process loses the endpoint, the lease is retained as
`state: "unavailable"` until explicitly released; it does not reserve VRAM.

Session heartbeats are opaque observer records. Besides `id`, `name`, `role`,
`mode` (`ask|preview|write`), `state`, `model`, `halt`, `files`, and `summary`,
they may carry `engine`, `plan_step`, `verification`, `changed_files`, and
`trace_ref`. Cameo stores and displays these fields; it never interprets them or
makes an agent decision from them.

In authenticated `GET /api/node` and hub heartbeats, a session with a lease also
contains a nested `lease` view (`endpoint_id`, `model`, `state`). The Deck uses
that relationship to show exactly which session owns a resident endpoint.

## Notes

- The JSON-RPC contract in `core/api` is the *typed* internal surface (Phase 2
  Unix-socket transport); this document describes the *HTTP* surface `cameod`
  serves today. Keep them in sync as the transport lands.
- Auth is fail-closed: a non-loopback bind is refused without the relevant key,
  so the GPU is never published unauthenticated.
