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
| `GET /healthz` | `{ "status": "ok" }` — process is up (F9). |
| `GET /readyz` | `{ "ready": true }` / `503` — can detect + plan (F9). |
| `GET /version` | `{ "name": "cameod", "version": "…" }` (F5). |
| `GET /metrics` | Prometheus text: daemon/endpoint/GPU gauges (F11). |
| `GET /` | The dashboard (HTML). |

## Console API (`/api`, console key)

| Method · Path | Purpose |
|---|---|
| `GET /api/gpus` | Detected GPUs, tiers, links, host RAM, bottleneck. |
| `GET /api/node` | This node's self-description — identity, topology, tiers, endpoints — for the fleet controller / k8s (F13). |
| `GET /api/engines` | Harness discovery: `/v1` base, `auth_required`, served models (F15). |
| `GET /api/models` | Cached models, aliases, cache dir. |
| `POST /api/models/gc` | Remove interrupted `.part` downloads (F12). |
| `DELETE /api/models/{name}` | Remove a cached model (F12). |
| `POST /api/plan` | Preview a placement plan for a model (no spawn). |
| `GET /api/servers` | List supervised endpoints (state, VRAM, restarts, uptime). |
| `POST /api/servers` | Plan + start an endpoint. `507` if it won't fit VRAM (F10). |
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

## Notes

- The JSON-RPC contract in `core/api` is the *typed* internal surface (Phase 2
  Unix-socket transport); this document describes the *HTTP* surface `cameod`
  serves today. Keep them in sync as the transport lands.
- Auth is fail-closed: a non-loopback bind is refused without the relevant key,
  so the GPU is never published unauthenticated.
