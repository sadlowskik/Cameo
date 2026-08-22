# Pointing a harness (Knossos) at Cameo — F15

Cameo is the substrate; a harness (Knossos) owns the agent loop. The bridge is
one stable, OpenAI-compatible engine URL per Cameo box or fleet. This is the
worked example: how a harness discovers Cameo's engines and points an engine slot
at them.

The standalone Rust Daedalus workspace is the harness source of truth. Cameo
does not vendor, build, or mutate agent code; its `daedalus` submodule is not an
integration target.

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
- **Tools and limits:** Cameo declares `tool_calls.native: false` and an
  `agent-managed` fallback in its versioned capability block. Treat
  `limits.max_request_bytes` as a hard wire limit; a `null`
  `max_completion_tokens` means the selected endpoint, not Cameo, controls
  generation length.
- **Auth:** when `auth_required` is true, present the serve key as
  `Authorization: Bearer <key>`. It is the same key for every model behind the one
  door.
- **Operator seam (self-host):** Knossos on the same box may speak HTTP to
  `/run/cameo/cameo.sock` (override with `CAMEO_SOCKET`). That connection is
  operator without a bearer key — load, stop, evict. Multi-tenant posture does
  not bind the socket. LAN TCP stays keyed.

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

That is the whole bridge: the Rust Knossos binary uses `--engine cameo` to discover `GET /api/engines`
(serve key is enough), fail-closes if the model is not resident, and — on the
host-only socket or with `CAMEO_CONSOLE_KEY` — `POST /api/servers` to ensure it.
A consumer key never loads VRAM. Cameo routes, supervises, auto-restarts (F9),
and VRAM-arbitrates (F10) behind `/v1`.

```bash
# The model is explicit: Cameo never guesses and surprises the box with a VRAM load.
export CAMEO_BASE_URL=http://box-a:9090/v1
export CAMEO_MODEL=qwen2.5-coder-7b

# Consumer credential: discovery and inference only.
export CAMEO_SERVE_KEY='<serve key>'

# Optional operator credential: allow deliberate cold-model provisioning.
# Omit this when Knossos must only reuse a resident model.
export CAMEO_CONSOLE_KEY='<operator key>'

daedalus --engine cameo task "add a regression test for the parser"
```

`CAMEO_CONSOLE_KEY` is intentionally separate from `CAMEO_SERVE_KEY`. In
self-host mode, a co-located harness can instead use `/run/cameo/cameo.sock`;
multi-tenant mode does not expose that privileged socket.

### Optional session residency

An agent that needs a long-lived model may opt into a session lease after it has
heartbeated a session and verified the model is running. This is intentionally
separate from normal discovery and inference: it avoids surprise GPU loads and
does not give a serve key any operator authority.

```bash
curl -X POST http://box-a:9090/api/sessions/agent-42/lease \
  -H "Authorization: Bearer $CAMEO_CONSOLE_KEY" \
  -d '{"model":"qwen2.5-coder-7b"}'
```

Refresh `POST /api/sessions` at least every 90 seconds, then `DELETE` this lease
when the task ends. A `409` means the model is not already running; provision it
through the normal operator flow first. `GET` returns `state: "unavailable"` if
an operator has stopped the backing endpoint, so the agent can choose whether to
re-ensure it. The current Rust engine does not need a lease for ordinary work;
adopt it only where eviction protection is worth the capacity trade-off.

Each session heartbeat may also report its selected `engine`, `plan_step`,
`verification`, `changed_files`, and `trace_ref`, alongside its existing mode,
state, model, halt reason, and summary. Cameo is intentionally an observer: it
renders these values in the Deck but does not infer whether the agent should
continue, write, or declare success.

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
