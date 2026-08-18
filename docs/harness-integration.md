# Pointing a harness (Knossos) at Cameo — F15

Cameo is the substrate; a harness (Knossos) owns the agent loop. The bridge is
one stable, OpenAI-compatible engine URL per Cameo box or fleet. This is the
worked example: how a harness discovers Cameo's engines and points an engine slot
at them.

## The contract

- **Engine API:** `POST http://<cameo>:9090/v1/chat/completions` (and
  `/v1/completions`, `/v1/embeddings`) — the unified gateway (F8) routes by the
  request's `model` field to the `llama-server` serving it.
- **Discovery:** `GET /api/engines` returns what a harness's engine slot needs:

  ```json
  { "node": "box-a", "openai_base_path": "/v1",
    "auth_required": true, "models": ["llama3.2-3b", "qwen2.5-0.5b"] }
  ```

  `GET /v1/models` is the OpenAI-native equivalent for clients that expect it.
- **Auth:** when `auth_required` is true, present the serve key as
  `Authorization: Bearer <key>`. It is the same key for every model behind the one
  door.

## One box

1. Stand a model up (CLI or the console):

   ```bash
   cameo pull llama3.2-3b
   curl -X POST http://box-a:9090/api/servers \
     -H "Authorization: Bearer $CAMEO_CONSOLE_KEY" \
     -d '{"model":"llama3.2-3b","params":3}'
   ```

2. Point the harness's engine slot at it — any OpenAI-compatible client works:

   ```python
   from openai import OpenAI
   engine = OpenAI(base_url="http://box-a:9090/v1", api_key=SERVE_KEY)
   engine.chat.completions.create(
       model="llama3.2-3b",
       messages=[{"role": "user", "content": "hello"}],
   )
   ```

That is the whole bridge: Knossos treats `http://box-a:9090/v1` as an OpenAI
provider, and Cameo routes, supervises, auto-restarts (F9), and VRAM-arbitrates
(F10) behind it.

## A fleet

Discover the fleet and let the planner choose a node, then point the harness at
the chosen node's `/v1`:

```bash
cameo fleet status --node box-a:9090 --node box-b:9090
cameo fleet place llama3.2-3b --params 3 --node box-a:9090 --node box-b:9090
```

`fleet place` reports which node fits the model (tightest fit). Stand the model up
there (as above), and the harness points at that node's `/v1`. When you outgrow
this, k8s consumes the same `/api/node` description — no rewrite.

## Why the resolver is not exposed raw

Cameo's `agents::resolve_agents` binds abstract agent specs to cloud-or-local
engines and, for a local agent, produces the exact `llama-server` command —
**which carries the serve key**. That plan is secret-bearing by construction, so
it is never serialized over HTTP. `/api/engines` is the safe, stable projection a
harness actually needs; the full resolver stays server-side, used by the
`cameo fleet` controller to stand agents up.
