# Cameo appliance — product spec

Status: locked draft, 2026-08-18. Spec only; implementation follows the build order.

One appliance, three layers. The **deck** is a fourth thing that only *looks*: it is a plugin host, not a fourth brain.

---

## Decisions locked

1. **Default brain = whatever is hooked up.** Prefer the model already resident on a Cameo node (Ollama / `llama-server` behind `/v1`). GUI on that node can load/unload a GGUF. Cloud models are first-class; save/pin them. No hidden default 27B.
2. **Kernel-level = privileged Cameo socket only.** Knossos the *host* talks to `/run/cameo/cameo.sock` on the local box. The model never gets syscalls, `/dev/kfd`, or `insmod`. No `cameo.ko` in v1.
3. **Deck v1 = tree + local git + GitHub remotes**, plus **compute** from Cameo plugins.
4. **Inference is a fleet.** Any machine that can run `cameod` is a node (bare metal, Proxmox VM, LXC with GPU passthrough). Each node has a dashboard at **that machine’s IP**. Multi-node GPUs are in-scope; multi-login humans can wait.
5. **The deck is a commander, not a plugin host.** v1 hardcodes two layers (Cameo HTTP compute, Knossos sessions). No plugin ABI until a third brain exists. LAN control is `cameod` HTTP only — no second Unix-socket API.

---

## Layers

| Layer | Owns | Must not own |
|---|---|---|
| **Cameo** (per node) | AMD detect, Vulkan/ROCm, pull/load GGUF, **one resident `/v1` per (model, node)**, VRAM, MoE offload, local dashboard, `/api/node` | Agent loop, git hosting, the deck |
| **Knossos** | Ask / preview / write sessions, tools, jail, Oracle, plan, engine slot | llama.cpp flags, expert placement, being a forge |
| **Deck** | Plugin host + one map (soldiers + compute + git) | Scheduling the GPU, deciding “done” |
| **Forge** (v2) | Self-hosted git + conversation objects | Replacing GitHub in v1 |

Existing code this sits on:

- `cameod` already binds `:9090`, serves a console at `/`, `GET /api/node`, `GET /api/engines`, `GET /api/gpus`, `/metrics`, load/unload via `/api/servers`.
- `cameo fleet` already polls a static node list and rebuilds `Cluster`.
- Knossos already has an OpenAI-compat client that can point at `http://<node>:9090/v1`.
- Gap: `moe-harness` is a stub. `resolve_agents` still opens one `llama-server` per agent (8100, 8101…). That is the efficiency bug. The deck does not exist yet.

---

## 1. Same device: normal chat and coding chat

One Knossos process. Modes, not apps.

| | Ask | Preview | Write |
|---|---|---|---|
| Feel | Claude chat | Claude Code dry-run | Claude Code |
| Tools | none / read-only | full, staged | full, disk |
| Oracle | never | syntax / staged | full ladder |
| Halt | model stop | request review | Oracle done |

Promote ask → preview → write with a click. Never silently demote.

Engine is independent of mode. Ask can be the resident model on this node. Write can be a saved Gemini pin, or a model on another Cameo node in the fleet.

---

## 2. Any machine is a Cameo node (including Proxmox)

Install `cameod` on:

- a Cameo ISO box
- an Arch/Debian guest on **Proxmox** with GPU passthrough (`/dev/dri`, `/dev/kfd`)
- an LXC only if passthrough is real; otherwise it is a CPU node and must say so

Each node:

- Binds the console to **`0.0.0.0:9090`** with `CAMEO_CONSOLE_KEY` (already refused without a key).
- Dashboard: `http://<that-machine-ip>:9090/` — cards, VRAM, loaded models, start/stop serve. This is **local to the node**, not a cloud control plane.
- Advertises itself: `GET /api/node` (topology, tiers, live endpoints), `GET /api/engines` (what a harness may call), `/metrics`.
- Serves inference: `http://<that-machine-ip>:9090/v1/chat/completions`.

The fleet is a **list of those IPs** (today: `cameo fleet --node a:9090 --node b:9090`). Later: Proxmox API or mDNS to discover guests tagged `cameo`. No central scheduler in v1 — the same `place_on_fleet` brain picks a node; you start the serve on that node.

Proxmox is a place Cameo *runs*, not a product Cameo becomes. We do not reimplement the Proxmox UI.

---

## 3. Deck = one map, two (or more) plugins

The deck is a shell: a canvas, selection, orders, a timeline. Everything on the map is a **plugin entity**.

```
deck (canvas + selection + orders)
  ├── plugin: cameo     → nodes, GPUs, VRAM, resident models, tok/s
  ├── plugin: knossos   → sessions (soldiers), plans, traces, Oracle
  └── later: git        → can live in knossos or a third plugin
```

### Plugin contract (v1)

A plugin is a small process or in-process module that:

1. **Declares** a name and a set of entity kinds (`node`, `gpu`, `model`, `session`, `file`, `commit`, `pr`).
2. **Snapshots** entities + edges on a poll or websocket (`id`, `kind`, `label`, `xy` hint, `meters` {vram, steps, tokens}, `state`).
3. **Accepts orders** the deck does not interpret (`load_model`, `prompt`, `cancel`, `attach_trace`).
4. **Never** calls the other plugin’s internals. If a soldier needs a GPU, Knossos asks Cameo via `/api/engines` / the socket, and the deck only *shows* both.

Cameo plugin paints: boxes (nodes), bars (VRAM / occupancy), chips (warm models). Click a node → that node’s dashboard in a pane, or open `http://<ip>:9090` in a tab.

Knossos plugin paints: units (sessions), journey (trace path over the file tree), fog (unread files), fight result (Oracle). Click a unit → transcript.

**One happy map:** compute underneath, soldiers on top, git as the terrain. Selecting a GPU shows which sessions are bound to it. Selecting a session shows which node/model it is using.

v1 cap: 8 write sessions. Ask sessions share the resident model and do not count as extra serves.

---

## 4. Engines: hook up, load, or save

When a session starts and the user did not pin:

1. Query Cameo plugins / `GET /api/engines` on known nodes — if something is **already loaded**, use it (prefer the local node).
2. Else last **saved** engine (node URL, or cloud provider+model+key ref).
3. Else the GUI: load a GGUF on a chosen node, or add a cloud pin. Never surprise-OOM.

**One resident serve per (model, node).** Ten chats on that node share `/v1`. Cluster: place the *model* once per node that needs it. Do not start a second copy of the same GGUF on the same card.

Cloud list lives in **Knossos**. Cameo stores local serves and optional secret pins; it does not grow a second Groq table.

---

## 5. Socket (the only “kernel”)

On each node, host-only: `gpu.status`, `engine.list`, `engine.ensure`, `engine.stop`, `engine.place`, `engine.metrics`.

Fleet control (`fleet.status` / `fleet.place`) can live on any node that has the node list, or on the deck host. The model never touches this socket.

---

## 6. Git, GitHub, conversations

Deck v1 maps: working tree, local git graph (mark commits that have a conversation), GitHub PR/issue pins for configured remotes.

Attach is explicit: “attach this chat to HEAD” → Lethe compact + git note + pointer to gitignored `.cameo/traces/<id>.jsonl`. Scrub secrets.

Forge (self-hosted GitHub alternative) is **v2**. v1 does not replace GitHub.

---

## 7. Build order

1. **Cameo node as a product** — document and finish: bind `0.0.0.0:9090` + key, dashboard shows card usage, `/api/node` + `/api/engines` + `/v1`, load/unload, one serve per model. Proxmox install notes (passthrough + `cameod`).
2. **Fleet list** — `cameo fleet` + deck Cameo plugin consuming `/api/node` from each IP.
3. **Knossos `cameo` provider** + ask / preview / write; hybrid router (pin → resident → saved cloud → fail).
4. **Deck shell + two plugins** — one map: nodes/GPUs + sessions/journeys. Then git graph. Then GitHub pins.
5. **Attach compact to commit.**
6. **MoE userspace** in Cameo; measure; still no kernel module.

---

## Non-goals (v1)

- Model at ring 0
- Bridgemind voice / 16-swarm
- Training-first
- Replacing GitHub
- Per-agent `llama-server`
- Multi-tenant SaaS
- Rebuilding Proxmox

## Key decisions

1. Three brains, one map. Deck only displays.
2. Deck v1 is two hardcoded layers, not a plugin framework.
3. Every Cameo box is a LAN appliance: dashboard + `/v1` on **that IP**.
4. Proxmox is a host for Cameo, not a Cameo feature.
5. Default engine = hooked-up resident, else saved, else GUI.
6. Socket-only OS access; one serve per (model, node).
7. Cloud providers stay in Knossos.
8. Forge is v2.
