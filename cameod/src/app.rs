//! The control-plane application: request routing and the glue that reuses
//! Cameo's detection and placement brain to answer the dashboard's API.
//!
//! Everything here is the same logic the `cameo` CLI runs — detect the topology,
//! classify tiers, plan a placement, build a `llama-server` command — exposed
//! over HTTP instead of a terminal. The daemon adds exactly one capability the
//! CLI lacks: it *keeps* the spawned process, via [`crate::supervisor`]. The
//! HTTP plumbing lives in [`crate::http`]; this module only decides what each
//! route means.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cameo_config::{Backend, Settings};
use cameo_gpu_detect::{
    classify_topology, detect_topology_or_cpu, Captures, OverrideDb, TierAssessment, Topology,
};
use cameo_placement::command::build_llama_server;
use cameo_placement::{plan as make_plan, ModelMeta, QuantLevel, Task};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::http::{Request, Response};
use crate::hub::{Farm, Registration};
use crate::sessions::{Board, Session};
use crate::supervisor::{StartError, StartRequest, Supervisor};

/// llama.cpp's HTTP server binary. As in the CLI, the backend selects the build,
/// not the name, so both tiers resolve to the same program today.
const SERVER_BINARY: &str = "llama-server";

/// Shared daemon state, handed to every request handler.
pub struct AppState {
    pub sup: Supervisor,
    /// Captured tool outputs for detection replay on a non-Linux host; empty
    /// means live detection (Linux). Cloned per detection so a request never
    /// holds a lock across the (pure) detection work.
    pub captures: Captures,
    /// Resolved settings (backend/HSA override/serve key), applied to every plan.
    pub settings: Settings,
    /// Role-tagged credentials. Operator keys gate the control surface (`/api`,
    /// `/hub`); consumer keys gate inference (`/v1`). The dashboard at `/` is
    /// always reachable so it can prompt for a key.
    pub keyring: crate::auth::KeyRing,
    /// Deployment posture: whether a co-located harness may be granted keyless
    /// operator power (self-host) or not (multi-tenant).
    pub posture: crate::auth::Posture,
    /// Short-lived detection snapshot for the machine-driven routes (`/readyz`,
    /// `/metrics`). Probes and scrapers fire on a cadence, and each live
    /// detection shells out to `lspci`/`rocminfo`/`rocm-smi` — caching for a few
    /// seconds turns that from per-probe subprocess churn into one run per
    /// window. Human-driven routes keep live detection.
    pub detect_cache: Mutex<Option<DetectSnapshot>>,
    /// Live harness sessions (Knossos soldiers) for the deck.
    pub board: Board,
    /// The fleet roster, when this daemon runs as a hub. Nodes phone home to
    /// `/hub/register`; empty and inert on a plain node.
    pub farm: Farm,
    /// True when this daemon is a hub: `/hub/*` enrollment is on and
    /// `GET /healthz` reports `hub: true`. `/` is always the one fleet map.
    pub hub_enabled: bool,
    /// The token a node must present to enroll (`POST /hub/register|heartbeat`).
    /// Required in hub mode — registration fails closed without it.
    pub farm_token: Option<String>,
    /// Whether `/v1` inference may be reached without any credential. True only
    /// for a loopback bind (dev) or an explicit `CAMEO_OPEN_INFERENCE` opt-in. On
    /// a routable bind it is false, so an operator-only key ring still gates `/v1`
    /// to that operator key instead of serving the GPU to anyone on the network.
    pub open_inference: bool,
}

/// A cached detection result: when it was taken, and what it saw.
pub type DetectSnapshot = (Instant, (Topology, Vec<TierAssessment>));

/// How long a cached detection snapshot may serve the probe/scrape routes.
/// Hardware does not change on this timescale; probe cadences do.
const DETECT_CACHE_TTL: Duration = Duration::from_secs(5);

/// The submitted description of a model to plan or serve. Sizing fields carry the
/// same defaults as the CLI's `ModelOpts`, so an omitted field means the same
/// thing in both front ends.
#[derive(Deserialize)]
struct ModelRequest {
    model: String,
    #[serde(default = "default_host")]
    host: String,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default)]
    params: Option<f64>,
    #[serde(default = "default_quant")]
    quant: String,
    #[serde(default)]
    moe: bool,
    #[serde(default = "default_context")]
    context: u32,
    #[serde(default)]
    layers: u32,
    /// `"vulkan"`, `"rocm"`, or `"auto"`; anything else (or absent) is auto.
    #[serde(default)]
    backend: Option<String>,
}

fn default_host() -> String {
    "127.0.0.1".into()
}
fn default_port() -> u16 {
    8080
}
fn default_quant() -> String {
    "Q4_K_M".into()
}
fn default_context() -> u32 {
    4096
}

impl ModelRequest {
    fn meta(&self) -> ModelMeta {
        let quant = QuantLevel::parse(&self.quant).unwrap_or(QuantLevel::Q4_K_M);
        let params = self
            .params
            .or_else(|| cameo_models::params_b_for(&self.model))
            .unwrap_or(7.0);
        let mut m = if self.moe {
            ModelMeta::moe(&self.model, params, quant)
        } else {
            ModelMeta::dense(&self.model, params, quant)
        };
        m.context_len = self.context;
        if self.layers > 0 {
            m.n_layers = self.layers;
        }
        m
    }

    fn backend(&self) -> Option<Backend> {
        match self.backend.as_deref() {
            Some("vulkan") => Some(Backend::Vulkan),
            Some("rocm") => Some(Backend::Rocm),
            Some("cpu") => Some(Backend::Cpu),
            Some("auto") | None => None,
            Some(_) => None,
        }
    }
}

/// Top-level dispatch. Returns a [`Response`] for every request; there is no
/// error path that escapes, so the HTTP layer only ever writes bytes.
pub fn route(state: &Arc<AppState>, req: &Request) -> Response {
    let segs = req.segments();

    // Unauthenticated, side-effect-free routes: the dashboard shell (so it can
    // prompt for a key) and the liveness/readiness probes (so k8s and the fleet
    // controller can reach them without the console key — F9/F13).
    if req.method == "GET" {
        match segs.as_slice() {
            // One page: this node's console plus the fleet map. Hub vs node is
            // `healthz.hub`; the HTML is the same so the map is one UI.
            [] => return Response::html(crate::dashboard::INDEX_HTML),
            ["healthz"] => {
                return Response::json(200, &json!({ "status": "ok", "hub": state.hub_enabled }))
            }
            ["readyz"] => {
                // Ready = the node can actually detect hardware and plan work.
                // Served from the short-lived cache: k8s probes on a cadence,
                // and readiness does not need a fresh subprocess sweep each time.
                return match detect_cached(state) {
                    Ok(_) => Response::json(200, &json!({ "ready": true })),
                    Err(_) => Response::json(503, &json!({ "ready": false })),
                };
            }
            // Prometheus scrape (F11). Gated by the console key when one is
            // configured: the default ISO/container bind is all-interfaces, and
            // an open /metrics there hands any LAN peer the model names, GPU
            // inventory and VRAM figures. Prometheus presents the key via its
            // standard `authorization` scrape config; a keyless dev daemon
            // stays open, mirroring every other route's rule.
            ["metrics"] => {
                if let Some(denied) = check_auth(state, req) {
                    return denied;
                }
                return metrics_response(state);
            }
            // Version, for the console's "update available?" check (F5).
            ["version"] => {
                return Response::json(
                    200,
                    &json!({ "name": "cameod", "version": env!("CARGO_PKG_VERSION") }),
                )
            }
            _ => {}
        }
    }

    // The OpenAI-compatible gateway (F8): one front door, routed by model name to
    // the right supervised llama-server. Gated by the *serve* key — the inference
    // credential — separate from the console key that gates /api.
    if segs.first() == Some(&"v1") {
        if let Some(denied) = check_serve_auth(state, req) {
            return denied;
        }
        return route_v1(state, req, &segs[1..]);
    }

    // The hub surface: node-facing enrollment (`register`/`heartbeat`, gated by
    // the farm token) and admin-facing fleet ops (`nodes`, push, gated by the
    // console key). Present only in hub mode.
    if segs.first() == Some(&"hub") {
        if !state.hub_enabled {
            return Response::error(404, "this daemon is not a hub");
        }
        return route_hub(state, req, &segs[1..]).no_store();
    }

    // Everything under /api is gated by the console key, when one is configured.
    // GET /api/engines is the harness discovery surface: a consumer (serve) key
    // is enough to see which models are resident; loading still needs operator.
    if segs.first() == Some(&"api") {
        if req.method == "GET" && segs.get(1) == Some(&"engines") && segs.len() == 2 {
            if let Some(denied) = check_engines_auth(state, req) {
                return denied;
            }
            return api_engines(state).no_store();
        }
        if let Some(denied) = check_auth(state, req) {
            return denied;
        }
        return route_api(state, req, &segs[1..]).no_store();
    }

    Response::error(404, "not found")
}

/// Compare a presented credential against the configured one in constant time.
/// A plain `==` short-circuits at the first differing byte, which leaks how much
/// of a guessed key matched through response timing. Only the length is
/// observable here, and that is not a secret.
fn key_matches(presented: Option<&str>, key: &str) -> bool {
    let Some(p) = presented else { return false };
    p.len() == key.len()
        && p.bytes()
            .zip(key.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// The bearer token on a request, if any.
fn bearer(req: &Request) -> Option<&str> {
    req.header("authorization")
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::trim)
}

/// Authenticate a `/v1` request. Any valid credential (consumer *or* operator) may
/// run inference. `None` = allowed.
///
/// `/v1` is only open without a credential where that is deliberately safe: a
/// loopback bind, or an explicit opt-in, *and* only when no consumer key is
/// configured to gate it. On a routable bind with an operator-only key ring the
/// gateway falls closed to that operator key — an unauthenticated public `/v1`
/// would serve the box's GPU to anyone who can reach the port.
fn check_serve_auth(state: &Arc<AppState>, req: &Request) -> Option<Response> {
    if req.from_unix && state.posture.allows_local_harness() {
        return None;
    }
    if state.open_inference && !state.keyring.requires_consumer() {
        return None;
    }
    if state.keyring.is_consumer_or_better(bearer(req)) {
        None
    } else {
        Some(Response::error(401, "missing or invalid api key"))
    }
}

/// The `/v1` OpenAI gateway (F8). `GET /v1/models` lists the served models; any
/// `POST /v1/*` (chat/completions, completions, embeddings) is routed by the
/// body's `model` field to the endpoint serving it and proxied.
fn route_v1(state: &Arc<AppState>, req: &Request, rest: &[&str]) -> Response {
    match (req.method.as_str(), rest) {
        ("GET", ["models"]) => {
            let data: Vec<Value> = state
                .sup
                .served_models()
                .into_iter()
                .map(|m| json!({ "id": m, "object": "model", "owned_by": "cameo" }))
                .collect();
            Response::json(200, &json!({ "object": "list", "data": data })).no_store()
        }
        ("POST", _) => gateway_proxy(state, req),
        _ => Response::error(404, "unknown /v1 route"),
    }
}

/// Route one gateway request: find the endpoint serving the body's `model`, mark
/// it used (for LRU residency), and proxy the call to its `llama-server`.
fn gateway_proxy(state: &Arc<AppState>, req: &Request) -> Response {
    let parsed = serde_json::from_slice::<Value>(&req.body).ok();
    let model = parsed
        .as_ref()
        .and_then(|v| v.get("model").and_then(Value::as_str).map(str::to_string));
    let Some(model) = model else {
        return Response::error(400, "request body must be JSON with a \"model\" field");
    };
    // OpenAI's `stream: true` asks for an SSE token stream; anything else buffers.
    let wants_stream = parsed
        .as_ref()
        .and_then(|v| v.get("stream").and_then(Value::as_bool))
        .unwrap_or(false);

    let Some((host, port, id)) = state.sup.endpoint_for_model(&model) else {
        return Response::error(
            404,
            format!("no running endpoint serves model '{model}'. Start one via POST /api/servers."),
        );
    };
    state.sup.touch(&id);

    let content_type = req
        .header("content-type")
        .unwrap_or("application/json")
        .to_string();
    let key = state.settings.serve_api_key.clone();

    if wants_stream {
        // Hand the socket to the proxy so upstream SSE frames are relayed as they
        // arrive instead of being buffered into one final blob. The closure owns
        // the request bytes because it outlives this call.
        let method = req.method.clone();
        let path = req.path.clone();
        let body = req.body.clone();
        return Response::streaming(move |w| {
            let upstream = crate::proxy::ProxyRequest {
                host: &host,
                port,
                method: &method,
                path: &path,
                content_type: &content_type,
                body: &body,
                backend_key: key.as_deref(),
            };
            crate::proxy::forward_streaming(&upstream, w)
        });
    }

    let upstream = crate::proxy::ProxyRequest {
        host: &host,
        port,
        method: &req.method,
        path: &req.path,
        content_type: &content_type,
        body: &req.body,
        backend_key: key.as_deref(),
    };
    match crate::proxy::forward(&upstream) {
        Ok(b) => Response::new(b.status, &b.content_type, b.body),
        Err(e) => Response::error(502, format!("upstream {host}:{port} unreachable: {e}")),
    }
}

/// Discover which models are resident. A serve key is enough; an operator key
/// also works. Keyless (no role configured) stays open, matching the rest of
/// `/api`. Loading a model still goes through [`check_auth`].
fn check_engines_auth(state: &Arc<AppState>, req: &Request) -> Option<Response> {
    if req.from_unix && state.posture.allows_local_harness() {
        return None;
    }
    if !state.keyring.requires_operator() && !state.keyring.requires_consumer() {
        return None;
    }
    if state.keyring.is_consumer_or_better(bearer(req)) {
        None
    } else {
        Some(Response::error(401, "missing or invalid api key"))
    }
}

/// Authenticate an operator request (`/api`, `/hub` admin). Requires an **operator**
/// credential — a consumer/friend key is deliberately refused here, so an inference
/// user can never manipulate VRAM or the fleet. `None` means allowed; no operator
/// key configured means the control surface is open (loopback dev).
fn check_auth(state: &Arc<AppState>, req: &Request) -> Option<Response> {
    // Host-only socket: a co-located harness on self-host is the operator.
    // Multi-tenant never binds that socket (see main.rs).
    if req.from_unix && state.posture.allows_local_harness() {
        return None;
    }
    if !state.keyring.requires_operator() {
        return None;
    }
    if state.keyring.is_operator(bearer(req)) {
        None
    } else {
        Some(Response::error(401, "operator credential required"))
    }
}

fn route_api(state: &Arc<AppState>, req: &Request, rest: &[&str]) -> Response {
    match (req.method.as_str(), rest) {
        ("GET", ["gpus"]) => api_gpus(state),
        ("GET", ["node"]) => api_node(state),
        ("GET", ["engines"]) => api_engines(state),
        ("GET", ["models"]) => api_models(),
        // Model-cache management (F12) surfaced for the console (F18).
        ("POST", ["models", "gc"]) => match cameo_models::gc_partials() {
            Ok(cleaned) => Response::json(200, &json!({ "cleaned": cleaned })),
            Err(e) => Response::error(500, e.to_string()),
        },
        ("DELETE", ["models", name]) => match cameo_models::remove(name) {
            Ok(path) => Response::json(200, &json!({ "removed": path.to_string_lossy() })),
            Err(e) => Response::error(404, e.to_string()),
        },
        ("POST", ["plan"]) => api_plan(state, req),
        ("GET", ["sessions"]) => Response::json(200, &json!({ "sessions": state.board.list() })),
        ("POST", ["sessions"]) => api_upsert_session(state, req),
        ("DELETE", ["sessions", id]) => {
            if state.board.remove(id) {
                Response::json(200, &json!({ "removed": id }))
            } else {
                Response::error(404, "no such session")
            }
        }
        ("GET", ["servers"]) => Response::json(200, &json!({ "servers": state.sup.list() })),
        ("POST", ["servers"]) => api_start_server(state, req),
        ("GET", ["servers", id]) => match state.sup.get(id) {
            Some(v) => Response::json(200, &v),
            None => Response::error(404, "no such endpoint"),
        },
        ("DELETE", ["servers", id]) => {
            if state.sup.stop(id) {
                Response::json(200, &json!({ "stopped": id }))
            } else {
                Response::error(404, "no such endpoint")
            }
        }
        _ => Response::error(404, "unknown API route"),
    }
}

// ---- hub (fleet) -----------------------------------------------------------

/// Authenticate a node-facing hub request against the farm token. Fails closed:
/// a hub with no farm token configured accepts no registrations.
fn check_farm_auth(state: &Arc<AppState>, req: &Request) -> Option<Response> {
    let Some(key) = state.farm_token.as_deref() else {
        return Some(Response::error(
            403,
            "hub is not accepting registrations (no farm token configured)",
        ));
    };
    let presented = req
        .header("authorization")
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::trim);
    if key_matches(presented, key) {
        None
    } else {
        Some(Response::error(401, "missing or invalid farm token"))
    }
}

/// The `/hub/*` surface. Enrollment routes are gated by the farm token; the
/// admin/roster routes by the console key (the same credential that guards
/// `/api`), since they read the fleet and drive nodes.
fn route_hub(state: &Arc<AppState>, req: &Request, rest: &[&str]) -> Response {
    match (req.method.as_str(), rest) {
        // A node phones home. Farm-token gated.
        ("POST", ["register"]) => {
            if let Some(denied) = check_farm_auth(state, req) {
                return denied;
            }
            match serde_json::from_slice::<Registration>(&req.body) {
                Ok(reg) => {
                    let node_id = state.farm.register(reg);
                    Response::json(200, &json!({ "node_id": node_id, "known": true }))
                }
                Err(e) => Response::error(400, format!("invalid registration: {e}")),
            }
        }
        // A node's liveness beat. Farm-token gated. `known:false` tells the agent
        // to re-register (the hub had dropped it for silence).
        ("POST", ["heartbeat"]) => {
            if let Some(denied) = check_farm_auth(state, req) {
                return denied;
            }
            let Ok(v) = serde_json::from_slice::<Value>(&req.body) else {
                return Response::error(400, "heartbeat body must be JSON");
            };
            let Some(node_id) = v.get("node_id").and_then(Value::as_str) else {
                return Response::error(400, "heartbeat needs a node_id");
            };
            let node = v.get("node").cloned();
            let known = state.farm.heartbeat(node_id, node);
            Response::json(200, &json!({ "known": known }))
        }
        // Admin: the fleet roster. Console-key gated.
        ("GET", ["nodes"]) => {
            if let Some(denied) = check_auth(state, req) {
                return denied;
            }
            Response::json(200, &json!({ "nodes": state.farm.list() }))
        }
        // Admin: forget a node.
        ("DELETE", ["nodes", id]) => {
            if let Some(denied) = check_auth(state, req) {
                return denied;
            }
            if state.farm.remove(id) {
                Response::json(200, &json!({ "removed": id }))
            } else {
                Response::error(404, "no such node")
            }
        }
        // Admin: push "serve this model" down to a node's own /api/servers — the
        // central-dashboard action that makes this HiveOS-shaped.
        ("POST", ["nodes", id, "servers"]) => {
            if let Some(denied) = check_auth(state, req) {
                return denied;
            }
            push_to_node(state, id, "POST", "/api/servers", Some(&req.body))
        }
        // Admin: stop a node's endpoint.
        ("DELETE", ["nodes", id, "servers", sid]) => {
            if let Some(denied) = check_auth(state, req) {
                return denied;
            }
            push_to_node(state, id, "DELETE", &format!("/api/servers/{sid}"), None)
        }
        // Harness delegation: route a task across the fleet by usage/card/model,
        // and (when `execute`) serve it on the chosen node. The whole point of the
        // fleet — one call and the box that should run the work does.
        ("POST", ["dispatch"]) => {
            if let Some(denied) = check_auth(state, req) {
                return denied;
            }
            api_dispatch(state, req)
        }
        _ => Response::error(404, "unknown hub route"),
    }
}

/// Call one node's authenticated `/api` over `curl`, using the callback address
/// and key it supplied at registration. `Ok(body)` on a 2xx, `Err((status, msg))`
/// otherwise — the hub is just an HTTP client here, exactly like `cameo fleet`.
fn node_call(
    state: &Arc<AppState>,
    node_id: &str,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<Vec<u8>, (u16, String)> {
    let Some((address, key)) = state.farm.push_target(node_id) else {
        return Err((404, format!("no such node '{node_id}'")));
    };
    // SSRF guard: a node's callback address is self-declared, so refuse to dial a
    // literal link-local address (169.254.0.0/16 hosts the cloud metadata service;
    // IPv6 fe80::/10). LAN, loopback, and hostnames are left alone — real nodes
    // live there (see `push_address_ok`).
    if !push_address_ok(&address) {
        return Err((
            400,
            format!("refusing to push to node '{node_id}': address {address} is link-local (possible SSRF to a metadata service)"),
        ));
    }
    let url = format!("http://{address}{path}");
    let mut cmd = std::process::Command::new("curl");
    // --max-time is kept under the inbound IO_TIMEOUT (30s) so a maximally-slow
    // node cannot consume the entire write budget of the client waiting on us.
    cmd.args(["-s", "--fail", "--max-time", "20", "-X", method]);
    if let Some(k) = &key {
        cmd.arg("-H").arg(format!("Authorization: Bearer {k}"));
    }
    if let Some(b) = body {
        cmd.args(["-H", "Content-Type: application/json", "-d"])
            .arg(String::from_utf8_lossy(b).as_ref());
    }
    cmd.arg(&url);
    match cmd.output() {
        Ok(out) if out.status.success() => Ok(out.stdout),
        Ok(out) => Err((
            502,
            format!(
                "node '{node_id}' at {address} rejected the request (curl exit {:?})",
                out.status.code()
            ),
        )),
        Err(e) => Err((502, format!("could not reach node '{node_id}': {e}"))),
    }
}

/// Relay an admin action to a node and turn the result into an HTTP response.
fn push_to_node(
    state: &Arc<AppState>,
    node_id: &str,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Response {
    match node_call(state, node_id, method, path, body) {
        Ok(out) => Response::new(200, "application/json; charset=utf-8", out),
        Err((status, msg)) => Response::error(status, msg),
    }
}

/// `POST /hub/dispatch` — the harness delegation route. Reconstructs the online
/// fleet, routes the task by usage/card/model, and either advises (`execute:false`)
/// or serves the model on the chosen node and returns its ready `/v1` endpoint.
fn api_dispatch(state: &Arc<AppState>, req: &Request) -> Response {
    let body: crate::dispatch::DispatchBody = match serde_json::from_slice(&req.body) {
        Ok(b) => b,
        Err(e) => return Response::error(400, format!("invalid dispatch body: {e}")),
    };

    let roster = state.farm.online_descriptions();
    let decision = match crate::dispatch::decide(&roster, &body) {
        Ok(d) => d,
        // Nothing eligible is a 409 (the fleet can't currently take the work),
        // distinct from a 400 (a malformed request).
        Err(e) => return Response::error(409, e.to_string()),
    };

    let choice = &decision.choice;
    let base = json!({
        "node_id": decision.node_id,
        "node": choice.node_name,
        "warm": choice.warm,
        "reason": choice.reason,
    });

    if !body.execute {
        let mut v = base;
        v["executed"] = json!(false);
        return Response::json(200, &v);
    }

    // Execute. A warm node already serves the model — no second llama-server.
    // A cold node gets a push to its own /api/servers.
    let address = state
        .farm
        .push_target(&decision.node_id)
        .map(|(a, _)| a)
        .unwrap_or_default();
    let endpoint = format!("http://{address}/v1");

    if choice.warm {
        let mut v = base;
        v["executed"] = json!(true);
        v["endpoint"] = json!(endpoint);
        v["note"] = json!("already serving; reused the resident endpoint");
        return Response::json(200, &v);
    }

    let serve_body = json!({
        "model": body.model, "host": "127.0.0.1", "port": body.port,
        "params": body.params.or_else(|| cameo_models::params_b_for(&body.model)).unwrap_or(7.0),
        "quant": body.quant, "moe": body.moe,
    })
    .to_string();
    match node_call(
        state,
        &decision.node_id,
        "POST",
        "/api/servers",
        Some(serve_body.as_bytes()),
    ) {
        Ok(out) => {
            let served: Value = serde_json::from_slice(&out).unwrap_or(json!({}));
            let mut v = base;
            v["executed"] = json!(true);
            v["endpoint"] = json!(endpoint);
            v["serve"] = served;
            Response::json(200, &v)
        }
        Err((status, msg)) => Response::error(
            status,
            format!(
                "routed to '{}' but the serve failed: {msg}",
                decision.node_id
            ),
        ),
    }
}

// ---- detection -------------------------------------------------------------

/// Map a detection error to an HTTP response, shared by every route that detects.
fn map_detect_err(e: cameo_gpu_detect::Error) -> Response {
    match e {
        cameo_gpu_detect::Error::UnsupportedOs => Response::error(
            501,
            "live GPU detection needs Linux. Start cameod with captured fixtures \
             (--lspci-file, …) to drive the console on this host.",
        ),
        cameo_gpu_detect::Error::NoGpu => Response::error(404, "no AMD GPU detected"),
        other => Response::error(500, other.to_string()),
    }
}

/// Detect + classify, returning the raw topology and per-card assessments. The
/// single source of detection for every route.
fn detect(state: &Arc<AppState>) -> Result<(Topology, Vec<TierAssessment>), Response> {
    let topo = detect_topology_or_cpu(&state.captures).map_err(map_detect_err)?;
    let assessments = classify_topology(&topo, &OverrideDb::embedded());
    Ok((topo, assessments))
}

/// [`detect`] behind the short-lived snapshot for the probe/scrape routes.
/// Failures are never cached — a box that becomes detectable is seen on the very
/// next probe, and a flapping one keeps reporting honestly.
fn detect_cached(state: &Arc<AppState>) -> Result<(Topology, Vec<TierAssessment>), Response> {
    {
        let cache = state.detect_cache.lock().unwrap();
        if let Some((at, snapshot)) = cache.as_ref() {
            if at.elapsed() < DETECT_CACHE_TTL {
                return Ok(snapshot.clone());
            }
        }
    }
    let fresh = detect(state)?;
    *state.detect_cache.lock().unwrap() = Some((Instant::now(), fresh.clone()));
    Ok(fresh)
}

/// The GPU report the dashboard renders (its specific shape).
fn detect_report(state: &Arc<AppState>) -> Result<Value, Response> {
    let (topo, assessments) = detect(state)?;
    Ok(json!({
        "gpus": assessments,
        "host_mem": topo.host_mem,
        "links": topo.links.iter().map(|l| json!({
            "a": l.a, "b": l.b, "kind": format!("{:?}", l.kind),
        })).collect::<Vec<_>>(),
        "bottleneck": topo.bottleneck_link().map(|k| format!("{k:?}")),
    }))
}

/// `GET /api/node` (F13): this box's full self-description — identity, the
/// serde-round-trippable topology, per-card tier assessments, and its live
/// endpoints. A `cameo fleet` controller (or k8s, or a harness) polls this to
/// build the `Cluster` the placement brain consumes; it is authenticated like the
/// rest of `/api`.
fn api_node(state: &Arc<AppState>) -> Response {
    let (topo, assessments) = match detect(state) {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    Response::json(200, &node_json(&topo, &assessments, state))
}

/// The `/api/node` body shape, in one place so `api_node` and the hub agent
/// ([`node_report`]) send byte-identical self-descriptions.
fn node_json(topo: &Topology, assessments: &[TierAssessment], state: &Arc<AppState>) -> Value {
    json!({
        "name": node_name(),
        "cameo_version": env!("CARGO_PKG_VERSION"),
        "topology": topo,
        "gpus": assessments,
        "endpoints": state.sup.list(),
        "sessions": state.board.list(),
    })
}

/// This node's current self-description for the hub agent, or `None` when
/// detection is unavailable (a non-Linux dev host) — in which case the agent
/// still enrolls, just without a hardware description.
pub fn node_report(state: &Arc<AppState>) -> Option<Value> {
    let (topo, assessments) = detect(state).ok()?;
    Some(node_json(&topo, &assessments, state))
}

/// This node's name: the OS hostname when we can read it, else a stable fallback.
pub(crate) fn node_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "cameo-node".into())
}

fn api_gpus(state: &Arc<AppState>) -> Response {
    match detect_report(state) {
        Ok(report) => Response::json(200, &report),
        Err(resp) => resp,
    }
}

/// `/metrics` (F11): the supervisor's endpoint metrics plus GPU-level gauges from
/// a detection snapshot. GPU metrics are best-effort — if detection is
/// unavailable (a non-Linux host with no fixtures), the endpoint metrics still
/// scrape cleanly.
fn metrics_response(state: &Arc<AppState>) -> Response {
    use crate::supervisor::esc;
    let mut body = state.sup.metrics();

    if let Ok((_topo, assessments)) = detect_cached(state) {
        body.push_str("# HELP cameo_gpu_count Number of detected GPUs.\n");
        body.push_str(&format!(
            "# TYPE cameo_gpu_count gauge\ncameo_gpu_count {}\n",
            assessments.len()
        ));
        body.push_str("# HELP cameo_gpu_vram_megabytes VRAM per GPU in MiB.\n");
        body.push_str("# TYPE cameo_gpu_vram_megabytes gauge\n");
        for (i, a) in assessments.iter().enumerate() {
            if let Some(vram) = a.gpu.vram_mb {
                body.push_str(&format!(
                    "cameo_gpu_vram_megabytes{{index=\"{i}\",model=\"{}\",tier=\"{}\"}} {vram}\n",
                    esc(&a.gpu.model),
                    a.tier.as_number(),
                ));
            }
        }
    }

    Response::new(
        200,
        "text/plain; version=0.0.4; charset=utf-8",
        body.into_bytes(),
    )
}

/// `GET /api/engines` (F15): the harness-facing engine descriptor. A harness
/// (Knossos) points its engine slot at this box by combining the host it queried
/// with `openai_base_path` and one of `models`, presenting the serve key when
/// `auth_required`. This is deliberately the *non-secret* surface: the full
/// agent-binding resolver (`agents::resolve_agents`) carries serve keys and stays
/// server-side, consumed by the `cameo fleet` controller — never serialized here.
fn api_engines(state: &Arc<AppState>) -> Response {
    let posture = match state.posture {
        crate::auth::Posture::SelfHost => "self-host",
        crate::auth::Posture::MultiTenant => "multi-tenant",
    };
    Response::json(
        200,
        &json!({
            "node": node_name(),
            "openai_base_path": "/v1",
            // /v1 needs a key whenever any consumer credential is configured (the
            // serve key is folded into the keyring as one), so a harness knows to
            // present one.
            "auth_required": state.keyring.requires_consumer(),
            "models": state.sup.served_models(),
            // Posture tells a harness whether the privileged local seam is on offer
            // here: only a self-host box grants a co-located harness GPU control.
            "posture": posture,
            "local_harness": state.posture.allows_local_harness(),
        }),
    )
}

fn api_models() -> Response {
    let aliases: Vec<Value> = cameo_models::aliases()
        .into_iter()
        .map(|a| {
            json!({
                "name": a.name,
                "repo": a.repo,
                "file": a.file,
                "params_b": cameo_models::params_b_for(a.name),
            })
        })
        .collect();
    Response::json(
        200,
        &json!({
            "aliases": aliases,
            "cached": cameo_models::cached_models(),
            "models_dir": cameo_models::models_dir().to_string_lossy(),
        }),
    )
}

// ---- planning & serving ----------------------------------------------------

/// Parse a JSON body into a [`ModelRequest`], or a `400` describing the problem.
fn parse_body(req: &Request) -> Result<ModelRequest, Response> {
    serde_json::from_slice(&req.body)
        .map_err(|e| Response::error(400, format!("invalid body: {e}")))
}

/// Plan a placement for a submitted model, returning `(plan-json, command)` or an
/// HTTP error. Shared by the preview route and the start route so a previewed
/// plan is exactly the one that would be served.
fn plan_for(
    state: &Arc<AppState>,
    body: &ModelRequest,
) -> Result<(cameo_placement::PlacementPlan, cameo_placement::CommandSpec), Response> {
    let (topo, assessments) = detect(state)?;

    // Fold the request's backend choice over the daemon's settings, matching the
    // CLI precedence (an explicit request beats the daemon default).
    let mut settings = state.settings.clone();
    if let Some(b) = body.backend() {
        settings.backend = Some(b);
    }

    let model = body.meta();
    let plan = make_plan(&topo, &assessments, &model, Task::Inference, &settings)
        .map_err(plan_error_response)?;

    let api_key = settings.serve_api_key.clone();
    let spec = build_llama_server(
        &plan,
        &model,
        // A path is only needed to actually spawn; the preview keeps the name.
        &cameo_models::resolve(&body.model).unwrap_or_else(|_| body.model.clone()),
        SERVER_BINARY,
        &body.host,
        body.port,
        api_key.as_deref(),
    );
    Ok((plan, spec))
}

fn api_plan(state: &Arc<AppState>, req: &Request) -> Response {
    let body = match parse_body(req) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    match plan_for(state, &body) {
        Ok((plan, spec)) => Response::json(
            200,
            &json!({
                "plan": plan,
                "command": { "program": spec.program, "args": spec.args, "shell": spec.display() },
            }),
        ),
        Err(resp) => resp,
    }
}

fn api_upsert_session(state: &Arc<AppState>, req: &Request) -> Response {
    match serde_json::from_slice::<Session>(&req.body) {
        Ok(s) => {
            let saved = state.board.upsert(s);
            Response::json(200, &serde_json::to_value(saved).unwrap_or(json!({})))
        }
        Err(e) => Response::error(400, format!("invalid session: {e}")),
    }
}

fn api_start_server(state: &Arc<AppState>, req: &Request) -> Response {
    let body = match parse_body(req) {
        Ok(b) => b,
        Err(resp) => return resp,
    };

    // Same safety rule as `cameo serve`: an unauthenticated endpoint bound to a
    // routable address publishes the GPU, so that combination is refused.
    if !is_loopback(&body.host) && state.settings.serve_api_key.is_none() {
        return Response::error(
            400,
            format!(
                "refusing to serve on {} without an endpoint api key. Set serve_api_key \
                 in the daemon config, or bind the endpoint to 127.0.0.1.",
                body.host
            ),
        );
    }

    // A real spawn needs the model on disk; name the fix if it is absent.
    if let Err(e) = cameo_models::resolve(&body.model) {
        return Response::error(400, e.to_string());
    }

    let (plan, spec) = match plan_for(state, &body) {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };

    // Residency inputs (F10): the box's usable VRAM and this model's GPU-resident
    // footprint, both from the planner. When VRAM is unknown, `0` disables
    // residency for this start rather than guessing.
    let vram_budget = if plan.budget.vram_known {
        plan.budget.vram_bytes
    } else {
        0
    };
    let vram_need = if vram_budget > 0 {
        // A model that fits keeps its true size; one that spills wants the whole
        // GPU, so cap the need at the budget.
        body.meta().total_bytes().min(vram_budget)
    } else {
        0
    };

    let start = StartRequest {
        model: body.model.clone(),
        host: body.host.clone(),
        port: body.port,
        backend: format!("{:?}", plan.backend),
        fits_vram: plan.fits_in_vram,
        notes: plan.notes.clone(),
        command: spec,
        vram_need,
        vram_budget,
    };
    match state.sup.start(start) {
        Ok(view) => Response::json(201, &view),
        Err(StartError::PortInUse(id)) => {
            Response::error(409, format!("endpoint {id} is already running"))
        }
        Err(StartError::WontFit(msg)) => Response::error(507, msg),
    }
}

// ---- helpers ---------------------------------------------------------------

/// Whether an address reaches this machine only (mirrors the CLI's rule).
fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// The host part of a `host:port` (or `[v6]:port`) address.
fn host_of(address: &str) -> &str {
    if let Some(rest) = address.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(address);
    }
    match address.rsplit_once(':') {
        Some((h, _)) => h,
        None => address,
    }
}

/// Whether the hub may dial a node's self-declared callback address. Nodes live on
/// the operator's LAN/VPN (or loopback when co-located), so those are fine; a
/// literal link-local address is refused because that range hosts the cloud
/// metadata service (IPv4 169.254.0.0/16, IPv6 fe80::/10) — a farm-token holder
/// must not be able to point the hub at instance credentials. Hostnames are left
/// to the operator's DNS, so this blocks the specific literal-IP SSRF, not LAN use.
fn push_address_ok(address: &str) -> bool {
    match host_of(address).parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => !(ip.is_link_local() || ip.is_unspecified()),
        Ok(std::net::IpAddr::V6(ip)) => {
            !(ip.is_unspecified() || (ip.segments()[0] & 0xffc0) == 0xfe80)
        }
        // Not a literal IP: a hostname the operator's DNS resolves — allowed.
        Err(_) => true,
    }
}

/// Map a placement error to an HTTP response with a stable, actionable message.
fn plan_error_response(e: cameo_placement::Error) -> Response {
    match e {
        cameo_placement::Error::TrainingUnsupported(tier) => Response::error(
            400,
            format!("training requires a Tier 1/2 (ROCm) GPU; top GPU is Tier {tier}"),
        ),
        e @ cameo_placement::Error::InsufficientMemory { .. } => {
            Response::error(400, e.to_string())
        }
        e @ cameo_placement::Error::InvalidModel(_) => Response::error(400, e.to_string()),
        other => Response::error(500, other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_splits_v4_v6_and_bare() {
        assert_eq!(host_of("10.0.0.2:9090"), "10.0.0.2");
        assert_eq!(host_of("[fe80::1]:9090"), "fe80::1");
        assert_eq!(host_of("box.local:9090"), "box.local");
        assert_eq!(host_of("box.local"), "box.local");
    }

    #[test]
    fn push_address_rejects_link_local_allows_lan_and_hosts() {
        // Link-local / metadata → refused.
        assert!(!push_address_ok("169.254.169.254:80"));
        assert!(!push_address_ok("169.254.0.1:9090"));
        assert!(!push_address_ok("[fe80::1]:9090"));
        assert!(!push_address_ok("0.0.0.0:9090"));
        // LAN, loopback (co-located self-host), public, and hostnames → allowed.
        assert!(push_address_ok("10.0.0.2:9090"));
        assert!(push_address_ok("192.168.1.5:9090"));
        assert!(push_address_ok("127.0.0.1:9090"));
        assert!(push_address_ok("box.local:9090"));
        assert!(push_address_ok("[2001:db8::1]:9090"));
    }
}
