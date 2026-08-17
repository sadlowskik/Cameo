# Cameo Internal API

The core service exposes one stable API; the CLI and (later) GUI are thin clients
over it. Neither ever touches a backend directly — that's what keeps them thin.

- **Transport:** JSON-RPC-style messages over a Unix domain socket
  (`/run/cameo/cameo.sock` by default). *Transport is implemented in Phase 2;*
  the message **types** (`core/api`) are the contract and exist now.
- **Versioning:** every message carries `version` (`API_VERSION`, currently `1`).
  Client and server compare it and refuse on mismatch.

## Request envelope
```json
{ "version": 1, "id": 42, "method": "model.run", "params": { "model": "qwen" } }
```
`method` selects the call; `params` is present only for methods that take
arguments. Unit methods (`gpu.status`, `install.plan`) omit `params`.

## Response envelope
```json
{ "version": 1, "id": 42, "status": "ok",    "data": { "tier": 2 } }
{ "version": 1, "id": 42, "status": "error", "code": "tier_unsupported",
  "message": "training requires Tier 1/2" }
```
`id` echoes the request. `status` is `ok` (with `data`) or `error` (with `code`
and `message`).

## Methods
| Method | Params | Purpose |
|--------|--------|---------|
| `gpu.status` | — | Detected GPU(s), tier, selected backend. |
| `model.run` | `{ model, backend? }` | Run inference (backend auto by tier unless overridden). |
| `model.quantize` | `{ model, level }` | Quantize to a level (e.g. `Q4_K_M`). |
| `train.start` | `{ config }` | Start training (Tier 1/2 only). |
| `install.plan` | — | Install plan for the detected hardware. |

## Error codes (CLI-aligned)
| Code | CLI exit | Meaning |
|------|----------|---------|
| `tier_unsupported` | 2 | Action needs a higher tier (e.g. train on Tier 3). |
| `not_implemented` | 3 | Backend not built yet (pre-Phase-2 stub). |
| `error` | 1 | Other errors (no GPU, bad input, I/O). |

## Evolving the contract
Add methods/params as new enum variants and optional fields (keeps old clients
working). Bump `API_VERSION` only for breaking changes. Because CLI and GUI share
these types, a change surfaces in both at compile time.
