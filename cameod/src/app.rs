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
use cameo_placement::{plan as make_plan, KvCacheType, ModelMeta, QuantLevel, Task};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::http::{Request, Response};
use crate::hub::{Farm, Registration};
use crate::sessions::{Board, Session, VramCapability};
use crate::supervisor::{LeaseError, StartError, StartRequest, Supervisor};

/// Resolve a backend-specific server binary. ROCm must never silently launch a
/// Vulkan build and report itself as ROCm; a missing ROCm binary is an explicit
/// spawn failure. Operators may override packaged paths without changing the
/// placement contract.
fn server_binary(backend: Backend) -> String {
    match backend {
        Backend::Rocm => {
            std::env::var("CAMEO_LLAMA_SERVER_ROCM").unwrap_or_else(|_| "llama-server-rocm".into())
        }
        Backend::Vulkan => {
            std::env::var("CAMEO_LLAMA_SERVER_VULKAN").unwrap_or_else(|_| "llama-server".into())
        }
        Backend::Cpu | Backend::Auto => {
            std::env::var("CAMEO_LLAMA_SERVER_CPU").unwrap_or_else(|_| "llama-server".into())
        }
    }
}

/// Shared daemon state, handed to every request handler.
pub struct AppState {
    pub drain: crate::drain::Drain,
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
    /// Atomic short-lived mesh admissions. This closes the hub-side TOCTOU gap
    /// between choosing a node and that node completing its local admission.
    pub admissions: crate::dispatch::AdmissionBook,
    /// Pending single-use device pairing codes. Codes are stored only as
    /// digests and expire after ten minutes.
    pub pairings: crate::pairing::PairingStore,
    /// Per-client request admission for brute-force and overload resistance.
    pub rate_limits: crate::rate_limit::RateLimiter,
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
#[derive(Deserialize, serde::Serialize)]
struct ModelRequest {
    model: String,
    #[serde(default)]
    model_sha256: Option<String>,
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
    /// Model-native ceiling from GGUF/HF metadata. The allocation is clamped
    /// to 80% before placement and launch.
    #[serde(default)]
    native_context: Option<u32>,
    #[serde(default = "default_slots")]
    slots: u16,
    #[serde(default = "default_kv_cache")]
    kv_cache: String,
    #[serde(default)]
    kv_heads: Option<u32>,
    #[serde(default)]
    head_dim: Option<u32>,
    #[serde(default = "default_batch")]
    batch: u32,
    #[serde(default = "default_ubatch")]
    ubatch: u32,
    #[serde(default = "default_true")]
    flash_attention: bool,
    #[serde(default = "default_cache_reuse")]
    cache_reuse: u32,
    #[serde(default)]
    cache_ram_mib: u32,
    #[serde(default = "default_true")]
    metrics: bool,
    #[serde(default)]
    slot_save_path: Option<String>,
    #[serde(default)]
    layers: u32,
    /// `"vulkan"`, `"rocm"`, or `"auto"`; anything else (or absent) is auto.
    #[serde(default)]
    backend: Option<String>,
}

/// Explicitly claim an already-running model for a live harness session.
/// Claiming never starts a model: only the operator's normal server API may do
/// that, so a malformed or stale session cannot surprise the box with a load.
#[derive(Deserialize)]
struct LeaseRequest {
    model: String,
}

/// A Knossos request to use Cameo-managed VRAM for a session. It reuses the
/// normal server-start fields, but defaults to no eviction until an operator
/// has seen and explicitly approved the impact in the Deck.
#[derive(Deserialize)]
struct VramEnsureRequest {
    #[serde(flatten)]
    server: ModelRequest,
    #[serde(default)]
    allow_evict: bool,
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
    0
}
fn default_slots() -> u16 {
    1
}
fn default_kv_cache() -> String {
    "q8_0".into()
}
fn default_batch() -> u32 {
    2048
}
fn default_ubatch() -> u32 {
    512
}
fn default_cache_reuse() -> u32 {
    256
}
fn default_true() -> bool {
    true
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
        let discovered = cameo_models::inference_meta_for(&self.model);
        m.native_context_len = self.native_context.or(discovered.map(|x| x.native_context));
        m.context_len = if self.context == 0 {
            m.native_context_len
                .map(|n| n.saturating_mul(80) / 100)
                .unwrap_or(4096)
        } else {
            self.context
        };
        m.parallel_slots = self.slots;
        m.kv_cache_type = KvCacheType::parse(&self.kv_cache);
        m.kv_heads = self.kv_heads.or(discovered.map(|x| x.kv_heads));
        m.head_dim = self.head_dim.or(discovered.map(|x| x.head_dim));
        m.batch_size = self.batch;
        m.ubatch_size = self.ubatch;
        m.flash_attention = self.flash_attention;
        m.cache_reuse = self.cache_reuse;
        m.cache_ram_mib = self.cache_ram_mib;
        m.metrics = self.metrics;
        m.slot_save_path = self.slot_save_path.clone();
        m.clamp_to_native_context();
        if self.layers > 0 {
            m.n_layers = self.layers;
        } else if let Some(meta) = discovered {
            m.n_layers = meta.layers;
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
    if let Some(denied) = check_request_rate(state, req, &segs) {
        return denied;
    }
    // A stale board entry is useful diagnostic history, but it must never keep
    // an endpoint non-evictable. This runs at request time, avoiding a
    // background thread and its shutdown/lifetime failure modes.
    release_stale_session_leases(state);

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
                if state.drain.draining() {
                    return Response::json(503, &json!({"ready":false, "reason":"draining"}));
                }
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
    // GET /api/engines and /api/capabilities are harness discovery surfaces: a
    // consumer (serve) key can inspect claims; loading still needs operator.
    if segs.first() == Some(&"api") {
        if req.method == "GET"
            && matches!(segs.get(1), Some(&"engines" | &"capabilities"))
            && segs.len() == 2
        {
            if let Some(denied) = check_engines_auth(state, req) {
                return denied;
            }
            return route_api(state, req, &segs[1..]).no_store();
        }
        if let Some(denied) = check_auth(state, req) {
            return denied;
        }
        return route_api(state, req, &segs[1..]).no_store();
    }

    Response::error(404, "not found")
}

fn check_request_rate(state: &Arc<AppState>, req: &Request, segs: &[&str]) -> Option<Response> {
    if req.from_unix {
        return None;
    }
    let client = req
        .peer_ip
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown".into());
    let authenticated = state.keyring.role_of(bearer(req)).is_some();
    let pairing = segs == ["hub", "pair"];
    let protected =
        segs.first() == Some(&"api") || segs.first() == Some(&"hub") || segs.first() == Some(&"v1");
    let intentionally_open =
        (segs.first() == Some(&"v1") && state.open_inference && !state.keyring.requires_consumer())
            || (segs.first() == Some(&"api") && !state.keyring.requires_operator());
    let (class, limit) = if pairing {
        ("pair", crate::rate_limit::PAIRING_REQUESTS_PER_WINDOW)
    } else if protected && !authenticated && !intentionally_open {
        ("auth", crate::rate_limit::INVALID_AUTH_REQUESTS_PER_WINDOW)
    } else if segs.first() == Some(&"v1") {
        (
            "inference",
            crate::rate_limit::INFERENCE_REQUESTS_PER_WINDOW,
        )
    } else if protected {
        ("control", crate::rate_limit::CONTROL_REQUESTS_PER_WINDOW)
    } else {
        ("public", crate::rate_limit::PUBLIC_REQUESTS_PER_WINDOW)
    };
    let key = format!("{client}:{class}");
    if state.rate_limits.allow(
        &key,
        limit,
        Duration::from_secs(crate::rate_limit::WINDOW_SECONDS),
    ) {
        None
    } else {
        Some(
            Response::error(429, "request rate limit exceeded")
                .with_header(
                    "Retry-After",
                    &crate::rate_limit::WINDOW_SECONDS.to_string(),
                )
                .no_store(),
        )
    }
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
/// The three declared POST routes (chat/completions, completions, embeddings) are routed by the
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
        ("POST", ["chat", "completions"] | ["completions"] | ["embeddings"]) => {
            let Some(permit) = state.drain.admit() else {
                return Response::json(503, &json!({"error":{"message":"node is draining", "type":"server_error", "code":"node_draining"}})).with_header("Retry-After", "5");
            };
            let mut response = gateway_proxy(state, req, rest, permit.generation());
            response.completion = Some(permit);
            response
        }
        _ => Response::error(404, "unknown /v1 route"),
    }
}

/// Route one gateway request: find the endpoint serving the body's `model`, mark
/// it used (for LRU residency), and proxy the call to its `llama-server`.
fn gateway_proxy(state: &Arc<AppState>, req: &Request, rest: &[&str], generation: u64) -> Response {
    let parsed = serde_json::from_slice::<Value>(&req.body).ok();
    let Some(parsed) = parsed else {
        return Response::error(400, "request body must be JSON with a \"model\" field");
    };
    if let Some(route) = crate::openai::route_from_path(rest) {
        if let Some(response) = crate::openai::reject_unsupported(route, &parsed) {
            return response;
        }
    }
    let model = parsed
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(model) = model else {
        return Response::error(400, "request body must be JSON with a \"model\" field");
    };
    if let Some(response) =
        crate::openai::reject_token_ceiling(&parsed, state.sup.context_tokens_for_model(&model))
    {
        return response;
    }
    // OpenAI's `stream: true` asks for an SSE token stream; anything else buffers.
    let wants_stream = parsed
        .get("stream")
        .and_then(Value::as_bool)
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
        let drain = state.drain.clone();
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
            crate::proxy::forward_streaming_controlled(&upstream, w, Some((&drain, generation)))
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
    match crate::proxy::forward_controlled(&upstream, Some((&state.drain, generation))) {
        Ok(b) => Response::new(b.status, &b.content_type, b.body),
        Err(_) if state.drain.cancelled(generation) => Response::json(503, &json!({"error":{"message":"drain deadline exceeded", "code":"node_draining", "type":"server_error"}})).with_header("Retry-After", "5"),
        Err(_) => Response::json(
            502,
            &serde_json::json!({
                "error": { "message": "upstream unavailable or invalid response",
                    "type": "server_error", "code": "upstream_unavailable" }
            }),
        ),
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
    if state.drain.shutting_down() && req.method != "GET" {
        return Response::error(503, "daemon is shutting down");
    }
    if rest == ["drain"] {
        return match req.method.as_str() {
            "GET" => Response::json(200, &state.drain.status()),
            "DELETE" => Response::json(200, &state.drain.resume()),
            "POST" => {
                let body: Value = match serde_json::from_slice(&req.body) {
                    Ok(body) => body,
                    Err(_) => return Response::error(400, "drain requires a JSON object"),
                };
                if !body.is_object() {
                    return Response::error(400, "drain requires a JSON object");
                }
                let seconds = match body.get("deadline_seconds") {
                    None => 60,
                    Some(value) => match value.as_u64() {
                        Some(n @ 1..=3600) => n,
                        _ => {
                            return Response::error(
                                400,
                                "deadline_seconds must be an integer from 1 to 3600",
                            )
                        }
                    },
                };
                Response::json(202, &state.drain.begin(Duration::from_secs(seconds)))
            }
            _ => Response::error(405, "unsupported drain method"),
        };
    }

    match (req.method.as_str(), rest) {
        ("GET", ["gpus"]) => api_gpus(state),
        ("GET", ["node"]) => api_node(state),
        ("GET", ["engines"]) => api_engines(state, req),
        ("GET", ["capabilities"]) => api_capabilities(),
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
        ("GET", ["sessions"]) | ("GET", ["knossos", "sessions"]) => {
            Response::json(200, &json!({ "sessions": state.board.list() }))
        }
        ("POST", ["sessions"]) | ("POST", ["knossos", "sessions"]) => {
            api_upsert_session(state, req)
        }
        ("GET", ["sessions", id]) | ("GET", ["knossos", "sessions", id]) => {
            api_get_session(state, id)
        }
        ("DELETE", ["sessions", id]) | ("DELETE", ["knossos", "sessions", id]) => {
            if let Err(error) = state.sup.remove_session(id) {
                tracing::error!(%error, "cannot persist lease release");
                return Response::error(503, "lease storage unavailable; retry release");
            }
            if state.board.remove(id) {
                Response::json(200, &json!({ "removed": id }))
            } else {
                Response::error(404, "no such session")
            }
        }
        ("POST", ["sessions", id, "lease"]) | ("POST", ["knossos", "sessions", id, "lease"]) => {
            api_lease_session(state, req, id)
        }
        ("GET", ["sessions", id, "lease"]) => match state.sup.lease_status(id) {
            Some(lease) => Response::json(200, &lease),
            None => Response::error(404, "no lease for this session"),
        },
        ("DELETE", ["sessions", id, "lease"]) => match state.sup.release(id) {
            Ok(true) => Response::json(200, &json!({ "released": id })),
            Ok(false) => Response::error(404, "no lease for this session"),
            Err(error) => {
                tracing::error!(%error, "cannot persist lease release");
                Response::error(503, "lease storage unavailable; retry release")
            }
        },
        ("POST", ["knossos", "sessions", id, "vram"]) => api_ensure_session_vram(state, req, id),
        ("GET", ["knossos", "sessions", id, "vram"]) => api_session_vram(state, id),
        ("DELETE", ["knossos", "sessions", id, "vram"]) => api_release_session_vram(state, id),
        ("GET", ["servers"]) => Response::json(200, &json!({ "servers": state.sup.list() })),
        ("POST", ["servers"]) => api_start_server(state, req),
        ("GET", ["servers", id]) => match state.sup.get(id) {
            Some(v) => Response::json(200, &v),
            None => Response::error(404, "no such endpoint"),
        },
        ("DELETE", ["servers", id]) => match state.sup.stop(id) {
            Ok(true) => Response::json(200, &json!({ "stopped": id })),
            Ok(false) => Response::error(404, "no such endpoint"),
            Err(_) => Response::error(503, "cannot persist endpoint stop; no process was stopped"),
        },
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
        // Operator creates a short-lived one-time code, then transfers it to the
        // physical device out of band.
        ("POST", ["pairings"]) => {
            if let Some(denied) = check_auth(state, req) {
                return denied;
            }
            let label = serde_json::from_slice::<Value>(&req.body)
                .ok()
                .and_then(|value| {
                    value
                        .get("label")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            let Some(label) = label else {
                return Response::error(400, "pairing request needs a string label");
            };
            match state.pairings.begin(&label) {
                Ok(offer) => Response::json(201, &json!(offer)),
                Err(e) => Response::error(429, e),
            }
        }
        // A device redeems the one-time code. Validation precedes consumption so
        // a malformed callback cannot burn a legitimate operator-created code.
        ("POST", ["pair"]) => {
            #[derive(Deserialize)]
            struct PairRequest {
                code: String,
                registration: Registration,
            }
            let body: PairRequest = match serde_json::from_slice(&req.body) {
                Ok(body) => body,
                Err(e) => return Response::error(400, format!("invalid pairing body: {e}")),
            };
            if !push_address_ok(&body.registration.address) {
                return Response::error(400, "pairing requires an allowed HTTPS callback URL");
            }
            let Some(callback_key) = body.registration.key.as_deref() else {
                return Response::error(400, "paired node requires an operator callback key");
            };
            if let Err(e) = crate::auth::validate_secret("node callback key", callback_key) {
                return Response::error(400, e);
            }
            let requested_id = if body.registration.node_id.trim().is_empty() {
                body.registration.name.trim()
            } else {
                body.registration.node_id.trim()
            };
            if requested_id.is_empty() || requested_id.len() > 256 {
                return Response::error(400, "pairing requires a node id or name up to 256 bytes");
            }
            if state.farm.contains(requested_id) {
                return Response::error(
                    409,
                    "node id is already enrolled; remove it before replacement",
                );
            }
            let credential = match crate::pairing::issue_device_credential() {
                Ok(credential) => credential,
                Err(e) => return Response::error(503, e),
            };
            let label = match state.pairings.consume(&body.code) {
                Ok(label) => label,
                Err(e) => return Response::error(401, e),
            };
            match state.farm.pair(
                body.registration,
                crate::pairing::hash_secret(&credential),
                label,
            ) {
                Ok(node_id) => Response::json(
                    201,
                    &json!({
                        "node_id": node_id,
                        "device_credential": credential,
                        "credential_displayed_once": true,
                        "trust": "paired",
                    }),
                ),
                Err(e) => Response::error(409, e),
            }
        }
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
            let Ok(v) = serde_json::from_slice::<Value>(&req.body) else {
                return Response::error(400, "heartbeat body must be JSON");
            };
            let Some(node_id) = v.get("node_id").and_then(Value::as_str) else {
                return Response::error(400, "heartbeat needs a node_id");
            };
            if !state.farm.authenticate_paired(node_id, bearer(req)) {
                if let Some(denied) = check_farm_auth(state, req) {
                    return denied;
                }
            }
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
            match state.farm.remove(id) {
                Ok(true) => Response::json(200, &json!({ "removed": id })),
                Ok(false) => Response::error(404, "no such node"),
                Err(error) => {
                    Response::error(500, format!("could not persist revocation: {error}"))
                }
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
            format!("refusing to push to node '{node_id}': callback {address} is not an allowed HTTPS node URL"),
        ));
    }
    let url = format!("{}{path}", address.trim_end_matches('/'));
    // Keep the outbound timeout under the inbound IO_TIMEOUT (30s), and keep
    // credentials/body off the process command line (see `curl::json_request`).
    match crate::curl::json_request(&url, method, key.as_deref(), body, 20) {
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
    if let Err(e) = body.validate() {
        return Response::error(400, e);
    }

    let roster = state.farm.online_descriptions();
    let admission = match state.admissions.admit(&roster, &body) {
        Ok(d) => d,
        // Nothing eligible is a 409 (the fleet can't currently take the work),
        // distinct from a 400 (a malformed request).
        Err(e) => return Response::error(e.status(), e.to_string()),
    };
    let decision = &admission.dispatch;

    let choice = &decision.choice;
    let transport_security = match choice.trust {
        cameo_placement::TrustState::Paired => "paired_identity_https_callback_bearer",
        cameo_placement::TrustState::LegacyToken => "legacy_farm_token_https_callback_bearer",
        cameo_placement::TrustState::Untrusted => "untrusted",
    };
    let base = json!({
        "node_id": decision.node_id,
        "node": choice.node_name,
        "warm": choice.warm,
        "affinity": choice.affinity,
        "predicted_completion_ms": choice.predicted_completion_ms,
        "trust": choice.trust,
        "health": choice.health,
        "protocol_major": choice.protocol_major,
        "transport_security": transport_security,
        "admission": admission.lease,
        "idempotent_replay": admission.replayed,
        "reason": choice.reason,
    });

    if admission.replayed {
        let admission_state = admission.lease.as_ref().map(|lease| lease.state);
        return match admission_state {
            Some(crate::dispatch::AdmissionState::Committed) => {
                let mut v = base;
                v["executed"] = json!(true);
                if let Some((address, _)) = state.farm.push_target(&decision.node_id) {
                    v["endpoint"] = json!(format!("{}/v1", address.trim_end_matches('/')));
                }
                v["note"] = json!("idempotent replay of committed dispatch");
                Response::json(200, &v)
            }
            Some(crate::dispatch::AdmissionState::Reserved) => {
                let mut v = base;
                v["executed"] = json!(false);
                v["note"] = json!("matching dispatch is already in progress");
                Response::json(202, &v)
            }
            Some(crate::dispatch::AdmissionState::Failed) => Response::error(
                409,
                "matching request_id already failed; use a new request_id to retry",
            ),
            None => Response::error(409, "invalid replayed admission state"),
        };
    }

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
    let endpoint = format!("{}/v1", address.trim_end_matches('/'));

    if choice.warm {
        if let Some(lease) = admission.lease.as_ref() {
            state.admissions.finish(&lease.id, true);
        }
        let mut v = base;
        v["admission"]["state"] = json!("committed");
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
            if let Some(lease) = admission.lease.as_ref() {
                state.admissions.finish(&lease.id, true);
            }
            let served: Value = serde_json::from_slice(&out).unwrap_or(json!({}));
            let mut v = base;
            v["admission"]["state"] = json!("committed");
            v["executed"] = json!(true);
            v["endpoint"] = json!(endpoint);
            v["serve"] = served;
            Response::json(200, &v)
        }
        Err((status, msg)) => {
            if let Some(lease) = admission.lease.as_ref() {
                state.admissions.finish(&lease.id, false);
            }
            Response::error(
                status,
                format!(
                    "routed to '{}' but the serve failed: {msg}",
                    decision.node_id
                ),
            )
        }
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
    let sessions = sessions_with_leases(state.board.list(), |id| state.sup.lease_status(id));
    json!({
        "name": node_name(),
        "cameo_version": env!("CARGO_PKG_VERSION"),
        "topology": topo,
        "gpus": assessments,
        "endpoints": state.sup.list(),
        "sessions": sessions,
    })
}

/// Add a safe lease projection to each session for node/hub consumers. The
/// session board remains authoritative for agent state; the supervisor remains
/// authoritative for endpoint state.
fn sessions_with_leases(
    sessions: Vec<Value>,
    lease_for: impl Fn(&str) -> Option<Value>,
) -> Vec<Value> {
    sessions
        .into_iter()
        .map(|mut session| {
            if let Some(id) = session.get("id").and_then(Value::as_str) {
                if let Some(lease) = lease_for(id) {
                    session["lease"] = lease;
                }
            }
            session
        })
        .collect()
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
fn api_engines(state: &Arc<AppState>, req: &Request) -> Response {
    let posture = match state.posture {
        crate::auth::Posture::SelfHost => "self-host",
        crate::auth::Posture::MultiTenant => "multi-tenant",
    };
    let include_vram = operator_view(state, req);
    Response::json(
        200,
        &engine_descriptor(
            &node_name(),
            state.keyring.requires_consumer(),
            state.sup.served_models(),
            state.sup.engine_profiles(include_vram),
            posture,
            state.posture.allows_local_harness(),
        ),
    )
}

/// The single versioned product capability document. It intentionally includes
/// planned and unsupported features so clients can distinguish absence from an
/// old server or a transient failure.
fn api_capabilities() -> Response {
    Response::json(200, &json!(cameo_api::capability_manifest()))
}

/// The stable, non-secret description a harness needs to use a Cameo node.
///
/// Keep the original fields flat: early harnesses consume `models` as a string
/// list. The versioned capability block is additive, so they can safely ignore
/// it while newer clients use it to reject a node that cannot meet their needs.
fn engine_descriptor(
    node: &str,
    auth_required: bool,
    models: Vec<String>,
    model_profiles: Vec<Value>,
    posture: &str,
    local_harness: bool,
) -> Value {
    let engine_state = if models.is_empty() { "idle" } else { "ready" };
    json!({
        "node": node,
        "engine_state": engine_state,
        "openai_base_path": "/v1",
        "auth_required": auth_required,
        "models": models,
        "model_profiles": model_profiles,
        "posture": posture,
        "local_harness": local_harness,
        "contract_version": "cameo-engine/v1",
        "product_capabilities": cameo_api::capability_manifest(),
        "capabilities": {
            "chat_completions": true,
            "completions": true,
            "embeddings": true,
            "streaming": true,
            "tool_calls": {
                "native": false,
                "fallback": "agent-managed",
            },
            "session_board": true,
            "operator_ensure": true,
            "operator_actions": ["ensure", "stop", "inspect"],
            "knossos": {
                "session_control": true,
                "vram_capability": true,
                "vram_api_path": "/api/knossos/sessions/{id}/vram",
                "no_surprise_eviction": true,
            },
        },
        "limits": {
            "max_request_bytes": crate::http::MAX_REQUEST_BODY_BYTES,
            "max_completion_tokens": null,
            "rate_window_seconds": crate::rate_limit::WINDOW_SECONDS,
            "public_requests_per_window": crate::rate_limit::PUBLIC_REQUESTS_PER_WINDOW,
            "invalid_auth_requests_per_window": crate::rate_limit::INVALID_AUTH_REQUESTS_PER_WINDOW,
            "inference_requests_per_window": crate::rate_limit::INFERENCE_REQUESTS_PER_WINDOW,
            "control_requests_per_window": crate::rate_limit::CONTROL_REQUESTS_PER_WINDOW,
            "pairing_requests_per_window": crate::rate_limit::PAIRING_REQUESTS_PER_WINDOW,
        },
        "session_api_path": "/api/sessions",
        "knossos_session_api_path": "/api/knossos/sessions",
        "operator_api_path": "/api/servers",
    })
}

/// Whether this request may see operator-only capacity facts. A consumer can
/// discover and call models, but not inventory the box's VRAM. Keyless local
/// development remains open because there is no credential boundary to honor.
fn operator_view(state: &Arc<AppState>, req: &Request) -> bool {
    (req.from_unix && state.posture.allows_local_harness())
        || state.keyring.is_operator(bearer(req))
        || (!state.keyring.requires_operator() && !state.keyring.requires_consumer())
}

fn api_models() -> Response {
    let aliases: Vec<Value> = cameo_models::aliases()
        .into_iter()
        .map(|a| {
            json!({
                "name": a.name,
                "repo": a.repo,
                "file": a.file,
                "sha256": a.sha256,
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
) -> Result<
    (
        cameo_placement::PlacementPlan,
        cameo_placement::CommandSpec,
        ModelMeta,
    ),
    Response,
> {
    let (topo, assessments) = detect(state)?;

    // Fold the request's backend choice over the daemon's settings, matching the
    // CLI precedence (an explicit request beats the daemon default).
    let mut settings = state.settings.clone();
    if let Some(b) = body.backend() {
        settings.backend = Some(b);
    }

    let model_path = cameo_models::resolve(&body.model).unwrap_or_else(|_| body.model.clone());
    let mut model = body
        .meta()
        .with_file_size(std::path::Path::new(&model_path));
    if let Ok(Some(meta)) = cameo_models::inspect_gguf(std::path::Path::new(&model_path)) {
        if body.native_context.is_none() {
            model.native_context_len = Some(meta.native_context);
        }
        if body.context == 0 {
            model.context_len = meta.native_context.saturating_mul(80) / 100;
        }
        if body.layers == 0 {
            model.n_layers = meta.layers;
        }
        if body.kv_heads.is_none() {
            model.kv_heads = Some(meta.kv_heads);
        }
        if body.head_dim.is_none() {
            model.head_dim = Some(meta.head_dim);
        }
        model.clamp_to_native_context();
    }
    let plan = make_plan(&topo, &assessments, &model, Task::Inference, &settings)
        .map_err(plan_error_response)?;

    let api_key = settings.serve_api_key.clone();
    let spec = build_llama_server(
        &plan,
        &model,
        // A path is only needed to actually spawn; the preview keeps the name.
        &model_path,
        &server_binary(plan.backend),
        &body.host,
        body.port,
        api_key.as_deref(),
    );
    Ok((plan, spec, model))
}

fn api_plan(state: &Arc<AppState>, req: &Request) -> Response {
    let body = match parse_body(req) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    match plan_for(state, &body) {
        Ok((plan, spec, _)) => Response::json(
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
            let saved = match state
                .board
                .upsert_durable(s, |session| state.sup.persist_session(session))
            {
                Ok(saved) => saved,
                Err(error) => {
                    tracing::error!(%error, "cannot persist session identity");
                    return Response::error(503, "session storage unavailable; retry heartbeat");
                }
            };
            if let Err(error) = state.sup.renew_lease(&saved.id) {
                tracing::error!(%error, "cannot persist lease heartbeat");
                return Response::error(
                    503,
                    "lease heartbeat storage unavailable; retry heartbeat",
                );
            }
            Response::json(200, &serde_json::to_value(saved).unwrap_or(json!({})))
        }
        Err(e) => Response::error(400, format!("invalid session: {e}")),
    }
}

fn api_get_session(state: &Arc<AppState>, session_id: &str) -> Response {
    let Some(session) = state.board.get(session_id) else {
        return Response::error(404, "no such session");
    };
    let mut view = serde_json::to_value(session).unwrap_or(json!({}));
    if let Some(lease) = state.sup.lease_status(session_id) {
        view["lease"] = lease;
    }
    Response::json(200, &view)
}

/// Claim an already-running model for a known session. This is deliberately a
/// separate opt-in from session heartbeats: reporting a session never reserves
/// VRAM, and a session cannot claim a model it has not actually observed live.
fn api_lease_session(state: &Arc<AppState>, req: &Request, session_id: &str) -> Response {
    if !state.board.contains(session_id) {
        return Response::error(404, "no such session");
    }
    let body: LeaseRequest = match serde_json::from_slice::<LeaseRequest>(&req.body) {
        Ok(body) if !body.model.trim().is_empty() => body,
        Ok(_) => return Response::error(400, "lease needs a non-empty model"),
        Err(e) => return Response::error(400, format!("invalid lease body: {e}")),
    };
    match state.sup.lease(session_id, &body.model) {
        Ok(lease) => Response::json(201, &lease),
        Err(LeaseError::Persistence(error)) => {
            tracing::error!(%error, "cannot persist lease");
            Response::error(503, "lease storage unavailable; retry claim")
        }
        Err(LeaseError::Unavailable(model)) => Response::error(
            409,
            format!("model '{model}' is unavailable; start it before claiming a lease"),
        ),
    }
}

/// Ensure a model for one Knossos session and immediately claim it. This is the
/// only Knossos-facing route that may load VRAM. It is operator-gated by `/api`
/// and refuses an implicit eviction even when the victim is otherwise eligible.
fn api_ensure_session_vram(state: &Arc<AppState>, req: &Request, session_id: &str) -> Response {
    if !state.board.contains(session_id) {
        return Response::error(404, "no such live session");
    }
    let body: VramEnsureRequest = match serde_json::from_slice::<VramEnsureRequest>(&req.body) {
        Ok(body) if !body.server.model.trim().is_empty() => body,
        Ok(_) => return Response::error(400, "vram ensure needs a non-empty model"),
        Err(e) => return Response::error(400, format!("invalid vram ensure body: {e}")),
    };
    let model = body.server.model.clone();
    let allow_evict = body.allow_evict;

    // Reuse is safer and faster than provisioning: neither VRAM arbitration nor
    // a second model process is involved.
    let existing_claim = state.sup.lease(session_id, &model);
    if let Err(LeaseError::Persistence(error)) = &existing_claim {
        tracing::error!(%error, "cannot persist lease");
        return Response::error(503, "lease storage unavailable; retry claim");
    }
    if let Ok(lease) = existing_claim {
        let endpoint_id = lease["endpoint_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let session = state.board.record_vram(
            session_id,
            VramCapability {
                status: "resident".into(),
                action: "ensure".into(),
                model: model.clone(),
                endpoint_id,
                impact: "reused resident model; no eviction".into(),
                evicts: Vec::new(),
            },
        );
        return Response::json(
            200,
            &json!({ "session": session, "lease": lease, "reused": true }),
        );
    }

    let response = start_server_with_eviction(state, body.server, allow_evict);
    if response.status < 200 || response.status >= 300 {
        let detail: Value = serde_json::from_slice(&response.body).unwrap_or(json!({}));
        let evicts = detail
            .get("evicts")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        state.board.record_vram(
            session_id,
            VramCapability {
                status: "blocked".into(),
                action: "ensure".into(),
                model,
                endpoint_id: String::new(),
                impact: detail
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Cameo could not admit the model")
                    .to_string(),
                evicts,
            },
        );
        return response;
    }

    let endpoint: Value = serde_json::from_slice(&response.body).unwrap_or(json!({}));
    match state.sup.lease(session_id, &model) {
        Ok(lease) => {
            let endpoint_id = lease["endpoint_id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let session = state.board.record_vram(
                session_id,
                VramCapability {
                    status: "resident".into(),
                    action: "ensure".into(),
                    model,
                    endpoint_id,
                    impact: if allow_evict {
                        "loaded after explicit operator-approved VRAM admission".into()
                    } else {
                        "loaded without evicting another endpoint".into()
                    },
                    evicts: Vec::new(),
                },
            );
            Response::json(
                201,
                &json!({ "session": session, "endpoint": endpoint, "lease": lease, "reused": false }),
            )
        }
        Err(LeaseError::Persistence(error)) => {
            tracing::error!(%error, "cannot persist lease");
            Response::error(503, "lease storage unavailable; retry claim")
        }
        Err(LeaseError::Unavailable(_)) => {
            state.board.record_vram(
                session_id,
                VramCapability {
                    status: "blocked".into(),
                    action: "ensure".into(),
                    model,
                    endpoint_id: String::new(),
                    impact: "endpoint did not become ready; inspect Cameo servers".into(),
                    evicts: Vec::new(),
                },
            );
            Response::error(
                502,
                "model process did not become ready; inspect /api/servers",
            )
        }
    }
}

fn api_session_vram(state: &Arc<AppState>, session_id: &str) -> Response {
    let Some(session) = state.board.get(session_id) else {
        return Response::error(404, "no such session");
    };
    Response::json(
        200,
        &json!({ "session_id": session_id, "capability": session.vram, "lease": state.sup.lease_status(session_id) }),
    )
}

fn api_release_session_vram(state: &Arc<AppState>, session_id: &str) -> Response {
    if state.board.get(session_id).is_none() {
        return Response::error(404, "no such session");
    }
    if let Err(error) = state.sup.release(session_id) {
        tracing::error!(%error, "cannot persist lease release");
        return Response::error(503, "lease storage unavailable; retry release");
    }
    let session = state.board.record_vram(
        session_id,
        VramCapability {
            status: "released".into(),
            action: "release".into(),
            model: String::new(),
            endpoint_id: String::new(),
            impact: "released the residency claim; the endpoint may remain warm".into(),
            evicts: Vec::new(),
        },
    );
    Response::json(200, &json!({ "session": session, "released": session_id }))
}

fn release_stale_session_leases(state: &Arc<AppState>) {
    for session_id in state.board.stale_ids() {
        if let Err(error) = state.sup.release(&session_id) {
            tracing::error!(%error, "cannot release stale session lease");
        }
    }
}

/// Replan persisted requests against current hardware, files and credentials.
/// Never replay saved command lines, live lease readiness or PIDs, and never evict on recovery.
pub fn recover_endpoints(state: &Arc<AppState>) {
    for (id, intent) in state.sup.recovery_intents() {
        let attempts = intent
            .get("_recovery_attempts")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if attempts >= 5 {
            tracing::warn!(%id, "recovery retry budget exhausted; explicit operator start required");
            continue;
        }
        let body: ModelRequest = match serde_json::from_value(intent) {
            Ok(body) => body,
            Err(_) => {
                tracing::error!(%id, "invalid persisted endpoint configuration");
                continue;
            }
        };
        let host = match body.host.as_str() {
            "0.0.0.0" => "127.0.0.1",
            "::" => "::1",
            other => other,
        };
        // An occupied address is ambiguous: never adopt/kill an unknown process
        // or announce it as healthy based only on its port.
        if std::net::TcpListener::bind((host, body.port)).is_err() {
            tracing::warn!(%id, "recovery refused: endpoint address is occupied or unavailable");
            continue;
        }
        let response = start_server_inner(state, body, false, attempts + 1);
        if response.status >= 300 {
            tracing::warn!(%id, status = response.status, "endpoint recovery requires operator action");
        }
    }
}

fn api_start_server(state: &Arc<AppState>, req: &Request) -> Response {
    let body = match parse_body(req) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    start_server_with_eviction(state, body, true)
}

/// Start the baked-in starter model so the playground works on first open.
///
/// `CAMEO_AUTOSTART_MODEL` selects the alias (`none` / `0` / empty disables).
/// Default is `qwen2.5-0.5b`. No-ops when the GGUF is not on disk.
pub fn maybe_autostart(state: &Arc<AppState>) {
    // An operator's durable stop must not be undone by starter defaults.
    if state.sup.has_history() {
        return;
    }
    let model = match std::env::var("CAMEO_AUTOSTART_MODEL") {
        Ok(s) if s.is_empty() || s == "none" || s == "0" => return,
        Ok(s) => s,
        Err(_) => "qwen2.5-0.5b".into(),
    };
    if let Err(e) = cameo_models::resolve(&model) {
        tracing::info!(%model, %e, "autostart skipped (model not in cache)");
        return;
    }
    let body = ModelRequest {
        model: model.clone(),
        model_sha256: None,
        host: default_host(),
        port: default_port(),
        params: cameo_models::params_b_for(&model),
        quant: default_quant(),
        moe: false,
        context: default_context(),
        native_context: None,
        slots: default_slots(),
        kv_cache: default_kv_cache(),
        kv_heads: None,
        head_dim: None,
        batch: default_batch(),
        ubatch: default_ubatch(),
        flash_attention: default_true(),
        cache_reuse: default_cache_reuse(),
        cache_ram_mib: 0,
        metrics: default_true(),
        slot_save_path: None,
        layers: 0,
        backend: None,
    };
    let resp = start_server_with_eviction(state, body, true);
    if resp.status >= 200 && resp.status < 300 {
        tracing::info!(%model, "autostarted starter model");
    } else {
        tracing::warn!(%model, status = resp.status, "autostart failed");
    }
}

fn start_server_with_eviction(
    state: &Arc<AppState>,
    body: ModelRequest,
    allow_evict: bool,
) -> Response {
    start_server_inner(state, body, allow_evict, 0)
}

fn start_server_inner(
    state: &Arc<AppState>,
    mut body: ModelRequest,
    allow_evict: bool,
    recovery_attempts: u64,
) -> Response {
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
    let model_path = match cameo_models::resolve(&body.model) {
        Ok(path) => path,
        Err(error) => return Response::error(400, error.to_string()),
    };
    let digest = match cameo_models::file_sha256(std::path::Path::new(&model_path)) {
        Ok(digest) => digest,
        Err(_) => {
            return Response::error(
                400,
                "model integrity check failed; verify the model file before starting",
            )
        }
    };
    let curated = cameo_models::aliases()
        .into_iter()
        .find(|alias| alias.name == body.model);
    if body
        .model_sha256
        .as_ref()
        .is_some_and(|expected| !expected.eq_ignore_ascii_case(&digest))
        || curated
            .as_ref()
            .is_some_and(|alias| !alias.sha256.eq_ignore_ascii_case(&digest))
    {
        return Response::error(409, "model digest differs from the pinned artifact; restore the verified model before starting");
    }
    body.model_sha256 = Some(digest);

    let (plan, spec, model_meta) = match plan_for(state, &body) {
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
        let resolved = cameo_models::resolve(&body.model).ok();
        let measured = resolved
            .as_deref()
            .map(std::path::Path::new)
            .map(|path| model_meta.clone().with_file_size(path).total_bytes())
            .unwrap_or_else(|| model_meta.total_bytes());
        measured.min(vram_budget)
    } else {
        0
    };

    let mut intent = serde_json::to_value(&body).expect("validated model request serializes");
    intent["_recovery_attempts"] = json!(recovery_attempts);
    let start = StartRequest {
        intent,
        model: body.model.clone(),
        host: body.host.clone(),
        port: body.port,
        backend: format!("{:?}", plan.backend),
        fits_vram: plan.fits_in_vram,
        notes: plan.notes.clone(),
        context_tokens: model_meta.context_len,
        command: spec,
        vram_need,
        vram_budget,
        allow_evict,
    };
    match state.sup.start(start) {
        Err(StartError::ShuttingDown) => Response::error(503, "daemon is shutting down"),
        Err(StartError::Termination(error)) => {
            tracing::error!(%error, "endpoint eviction could not confirm exit");
            Response::error(
                503,
                "previous endpoint has not confirmed exit; inspect servers before retrying",
            )
        }
        Err(StartError::Persistence(error)) => {
            tracing::error!(%error, "endpoint intent persistence failed");
            Response::error(
                503,
                "cannot persist endpoint intent; no processes were changed",
            )
        }
        Ok(view) => Response::json(201, &view),
        Err(StartError::PortInUse(id)) => {
            Response::error(409, format!("endpoint {id} is already running"))
        }
        Err(StartError::WontFit(msg)) => Response::error(507, msg),
        Err(StartError::LeasedCapacity(msg)) => Response::error(409, msg),
        Err(StartError::EvictionRequired(ids)) => Response::json(
            409,
            &json!({
                "error": "loading this model would evict active unleased endpoints; operator confirmation is required",
                "status": 409,
                "requires_operator_confirmation": true,
                "evicts": ids,
            }),
        ),
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

/// The host part of a `host:port` (or `[v6]:port`) authority.
fn host_of(address: &str) -> &str {
    if let Some(rest) = address.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(address);
    }
    match address.rsplit_once(':') {
        Some((h, _)) => h,
        None => address,
    }
}

/// Extract the authority from the HTTPS callback base URL. Callback URLs may
/// have a single trailing slash but no credentials, path, query, or fragment.
fn callback_authority(address: &str) -> Option<&str> {
    let authority = address.strip_prefix("https://")?.trim_end_matches('/');
    if authority.is_empty()
        || authority.contains('/')
        || authority.contains('@')
        || authority.contains('?')
        || authority.contains('#')
    {
        return None;
    }
    Some(authority)
}

/// Whether the hub may dial a node's self-declared callback address. Nodes live on
/// the operator's LAN/VPN (or loopback when co-located), so those are fine; a
/// literal link-local address is refused because that range hosts the cloud
/// metadata service (IPv4 169.254.0.0/16, IPv6 fe80::/10) — a farm-token holder
/// must not be able to point the hub at instance credentials. Hostnames are left
/// to the operator's DNS, so this blocks the specific literal-IP SSRF, not LAN use.
fn push_address_ok(address: &str) -> bool {
    let Some(authority) = callback_authority(address) else {
        return false;
    };
    match host_of(authority).parse::<std::net::IpAddr>() {
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
        cameo_placement::Error::BackendUnsupported(message) => Response::error(400, message),
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

    fn contract_state() -> Arc<AppState> {
        Arc::new(AppState {
            drain: crate::drain::Drain::default(),
            sup: Supervisor::new(),
            captures: Captures::default(),
            settings: Settings::default(),
            keyring: crate::auth::KeyRing::new(vec![
                crate::auth::ApiKey {
                    key: "operator-key".into(),
                    role: crate::auth::Role::Operator,
                    label: "test operator".into(),
                },
                crate::auth::ApiKey {
                    key: "consumer-key".into(),
                    role: crate::auth::Role::Consumer,
                    label: "test consumer".into(),
                },
            ]),
            posture: crate::auth::Posture::MultiTenant,
            detect_cache: Mutex::new(None),
            board: Board::new(),
            farm: Farm::new(),
            admissions: crate::dispatch::AdmissionBook::new(),
            pairings: crate::pairing::PairingStore::new(),
            rate_limits: crate::rate_limit::RateLimiter::new(),
            hub_enabled: true,
            farm_token: None,
            open_inference: false,
        })
    }

    fn contract_request(method: &str, path: &str, key: Option<&str>, body: Value) -> Request {
        let mut headers = std::collections::HashMap::new();
        if let Some(key) = key {
            headers.insert("authorization".into(), format!("Bearer {key}"));
        }
        Request {
            method: method.into(),
            path: path.into(),
            query: std::collections::HashMap::new(),
            headers,
            body: serde_json::to_vec(&body).unwrap(),
            from_unix: false,
            peer_ip: Some("127.0.0.1".parse().unwrap()),
        }
    }

    fn response_json(response: Response) -> Value {
        serde_json::from_slice(&response.body).expect("JSON response")
    }

    #[test]
    fn drain_is_operator_controlled_and_keeps_health_available() {
        let state = contract_state();
        assert_eq!(
            route(
                &state,
                &contract_request("POST", "/api/drain", Some("consumer-key"), json!({}))
            )
            .status,
            401
        );
        assert!(!state.drain.draining());
        assert_eq!(
            route(
                &state,
                &contract_request(
                    "POST",
                    "/api/drain",
                    Some("operator-key"),
                    json!({"deadline_seconds":0})
                )
            )
            .status,
            400
        );
        assert_eq!(
            route(
                &state,
                &contract_request("POST", "/api/drain", Some("operator-key"), json!({}))
            )
            .status,
            202
        );
        let inference = route(
            &state,
            &contract_request(
                "POST",
                "/v1/chat/completions",
                Some("consumer-key"),
                json!({"model":"fixture"}),
            ),
        );
        assert_eq!(inference.status, 503);
        assert_eq!(response_json(inference)["error"]["code"], "node_draining");
        assert_eq!(
            route(
                &state,
                &contract_request("GET", "/healthz", None, json!({}))
            )
            .status,
            200
        );
        assert_eq!(
            route(&state, &contract_request("GET", "/readyz", None, json!({}))).status,
            503
        );
        assert_eq!(
            route(
                &state,
                &contract_request("DELETE", "/api/drain", Some("operator-key"), json!({}))
            )
            .status,
            200
        );
        assert!(!state.drain.draining());
    }

    #[test]
    fn changed_pinned_model_is_rejected_before_placement_or_spawn() {
        let state = contract_state();
        let path =
            std::env::temp_dir().join(format!("cameo-pinned-model-{}.gguf", std::process::id()));
        std::fs::write(&path, b"changed model bytes").unwrap();
        let response = route(
            &state,
            &contract_request(
                "POST",
                "/api/servers",
                Some("operator-key"),
                json!({ "model": path.to_string_lossy(), "model_sha256": "00".repeat(32) }),
            ),
        );
        std::fs::remove_file(path).unwrap();
        assert_eq!(response.status, 409);
        assert!(response_json(response)["error"]
            .as_str()
            .unwrap()
            .contains("digest"));
        assert!(state.sup.list().is_empty());
    }

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
        assert!(!push_address_ok("https://169.254.169.254:80"));
        assert!(!push_address_ok("https://169.254.0.1:9090"));
        assert!(!push_address_ok("https://[fe80::1]:9090"));
        assert!(!push_address_ok("https://0.0.0.0:9090"));
        assert!(!push_address_ok("http://10.0.0.2:9090"));
        assert!(!push_address_ok("https://user@box.local:9090"));
        assert!(!push_address_ok("https://box.local:9090/admin"));
        // LAN, loopback (co-located self-host), public, and hostnames → allowed.
        assert!(push_address_ok("https://10.0.0.2:9090"));
        assert!(push_address_ok("https://192.168.1.5:9090"));
        assert!(push_address_ok("https://127.0.0.1:9090"));
        assert!(push_address_ok("https://box.local:9090"));
        assert!(push_address_ok("https://[2001:db8::1]:9090/"));
    }

    #[test]
    fn engine_descriptor_keeps_the_legacy_shape_and_advertises_v1_capabilities() {
        let descriptor = engine_descriptor(
            "box-a",
            true,
            vec!["qwen2.5-coder-7b".into()],
            vec![json!({
                "model": "qwen2.5-coder-7b",
                "context_tokens": 32768,
            })],
            "self-host",
            true,
        );
        assert_eq!(descriptor["node"], "box-a");
        assert_eq!(descriptor["openai_base_path"], "/v1");
        assert_eq!(descriptor["models"], json!(["qwen2.5-coder-7b"]));
        assert_eq!(descriptor["contract_version"], "cameo-engine/v1");
        assert_eq!(
            descriptor["product_capabilities"]["contract_version"],
            "cameo-capabilities/v1"
        );
        assert_eq!(
            descriptor["product_capabilities"]["mesh"]["mutual_tls"]["available"],
            false
        );
        assert_eq!(descriptor["engine_state"], "ready");
        assert_eq!(descriptor["capabilities"]["streaming"], true);
        assert_eq!(descriptor["capabilities"]["tool_calls"]["native"], false);
        assert_eq!(descriptor["limits"]["max_request_bytes"], 1024 * 1024);
        assert_eq!(descriptor["limits"]["rate_window_seconds"], 60);
        assert_eq!(descriptor["limits"]["pairing_requests_per_window"], 10);
        assert_eq!(descriptor["limits"]["invalid_auth_requests_per_window"], 30);
        assert_eq!(descriptor["limits"]["inference_requests_per_window"], 600);
        assert_eq!(descriptor["limits"]["control_requests_per_window"], 300);
        assert_eq!(descriptor["limits"]["public_requests_per_window"], 300);
        assert_eq!(descriptor["model_profiles"][0]["context_tokens"], 32768);
        assert_eq!(descriptor["session_api_path"], "/api/sessions");
        assert_eq!(descriptor["operator_api_path"], "/api/servers");
        assert_eq!(
            descriptor["capabilities"]["knossos"]["vram_api_path"],
            "/api/knossos/sessions/{id}/vram"
        );
        assert_eq!(
            descriptor["capabilities"]["knossos"]["no_surprise_eviction"],
            true
        );
    }

    #[test]
    fn consumer_can_discover_and_probe_inference_but_cannot_operate_the_node() {
        let state = contract_state();

        let discovery = route(
            &state,
            &contract_request("GET", "/api/engines", Some("consumer-key"), json!({})),
        );
        assert_eq!(discovery.status, 200);
        let descriptor = response_json(discovery);
        assert_eq!(descriptor["contract_version"], "cameo-engine/v1");
        assert!(descriptor["model_profiles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|profile| profile.get("vram_bytes").is_none()));

        let capabilities = route(
            &state,
            &contract_request("GET", "/api/capabilities", Some("consumer-key"), json!({})),
        );
        assert_eq!(capabilities.status, 200);
        let capabilities = response_json(capabilities);
        assert_eq!(capabilities["contract_version"], "cameo-capabilities/v1");
        assert_eq!(
            capabilities["mesh"]["enrollment_transport"],
            "paired_bearer_or_legacy_farm_token"
        );

        let inference = route(
            &state,
            &contract_request(
                "POST",
                "/v1/chat/completions",
                Some("consumer-key"),
                json!({ "model": "not-running", "messages": [] }),
            ),
        );
        assert_eq!(inference.status, 404, "consumer auth reaches model routing");

        let undeclared = route(
            &state,
            &contract_request(
                "POST",
                "/v1/internal/admin",
                Some("consumer-key"),
                json!({ "model": "not-running" }),
            ),
        );
        assert_eq!(
            undeclared.status, 404,
            "the gateway never forwards undeclared backend paths"
        );

        let denied = route(
            &state,
            &contract_request("POST", "/api/sessions", Some("consumer-key"), json!({})),
        );
        assert_eq!(denied.status, 401, "consumer cannot mutate session state");

        let knossos_vram = route(
            &state,
            &contract_request(
                "POST",
                "/api/knossos/sessions/customer-path/vram",
                Some("consumer-key"),
                json!({ "model": "not-running" }),
            ),
        );
        assert_eq!(
            knossos_vram.status, 401,
            "consumer cannot reserve or alter VRAM through Knossos"
        );
    }

    #[test]
    fn gateway_rejects_unadvertised_openai_features_before_routing() {
        let state = contract_state();
        let tools = route(
            &state,
            &contract_request(
                "POST",
                "/v1/chat/completions",
                Some("consumer-key"),
                json!({
                    "model": "not-running",
                    "messages": [{"role":"user","content":"hi"}],
                    "tools": [{"type":"function","function":{"name":"x"}}]
                }),
            ),
        );
        assert_eq!(
            tools.status, 400,
            "unsupported features fail before 404 routing"
        );
        assert_eq!(
            response_json(tools)["error"]["code"],
            "unsupported_parameter"
        );
        let logprobs = route(
            &state,
            &contract_request(
                "POST",
                "/v1/chat/completions",
                Some("consumer-key"),
                json!({"model":"not-running","logprobs":true}),
            ),
        );
        assert_eq!(logprobs.status, 400);
        let ordinary = route(
            &state,
            &contract_request(
                "POST",
                "/v1/chat/completions",
                Some("consumer-key"),
                json!({"model":"not-running","messages":[]}),
            ),
        );
        assert_eq!(
            ordinary.status, 404,
            "supported bodies still reach model routing"
        );
    }

    #[test]
    fn repeated_invalid_credentials_are_rate_limited_per_client() {
        let state = contract_state();
        let request = contract_request("GET", "/api/node", Some("wrong-key"), json!({}));
        for _ in 0..30 {
            assert_eq!(route(&state, &request).status, 401);
        }
        let limited = route(&state, &request);
        assert_eq!(limited.status, 429);
        assert!(limited
            .extra_headers
            .iter()
            .any(|(name, value)| name == "Retry-After" && value == "60"));
    }

    #[test]
    fn operator_pairing_code_enrolls_one_device_and_issues_one_credential() {
        let state = contract_state();
        let offer = route(
            &state,
            &contract_request(
                "POST",
                "/hub/pairings",
                Some("operator-key"),
                json!({ "label": "office laptop" }),
            ),
        );
        assert_eq!(offer.status, 201);
        let code = response_json(offer)["code"].as_str().unwrap().to_string();

        let registration = json!({
            "node_id": "laptop-a",
            "name": "laptop-a",
            "address": "https://10.0.0.2:9090",
            "key": "node-operator-key",
            "node": null
        });
        let paired = route(
            &state,
            &contract_request(
                "POST",
                "/hub/pair",
                None,
                json!({ "code": code, "registration": registration }),
            ),
        );
        assert_eq!(paired.status, 201);
        let paired = response_json(paired);
        let credential = paired["device_credential"].as_str().unwrap();
        assert_eq!(credential.len(), 64);
        assert_eq!(paired["trust"], "paired");

        let heartbeat = route(
            &state,
            &contract_request(
                "POST",
                "/hub/heartbeat",
                Some(credential),
                json!({ "node_id": "laptop-a", "node": null }),
            ),
        );
        assert_eq!(heartbeat.status, 200);

        let replay = route(
            &state,
            &contract_request(
                "POST",
                "/hub/pair",
                None,
                json!({
                    "code": code,
                    "registration": {
                        "node_id": "laptop-b", "name": "laptop-b",
                        "address": "https://10.0.0.3:9090", "key": "another-node-key"
                    }
                }),
            ),
        );
        assert_eq!(replay.status, 401, "pairing codes are single use");
    }

    #[test]
    fn operator_session_lease_requires_a_live_model_and_never_loads_one() {
        let state = contract_state();
        let session = route(
            &state,
            &contract_request(
                "POST",
                "/api/sessions",
                Some("operator-key"),
                json!({ "id": "customer-path", "mode": "write", "model": "not-running" }),
            ),
        );
        assert_eq!(session.status, 200);

        let lease = route(
            &state,
            &contract_request(
                "POST",
                "/api/sessions/customer-path/lease",
                Some("operator-key"),
                json!({ "model": "not-running" }),
            ),
        );
        assert_eq!(lease.status, 409);
        assert!(state.sup.list().is_empty(), "claiming is not provisioning");
    }

    #[test]
    fn node_session_projection_keeps_unleased_sessions_and_attaches_owners() {
        let sessions = vec![
            json!({ "id": "leased", "name": "builder" }),
            json!({ "id": "ordinary", "name": "reviewer" }),
        ];
        let projected = sessions_with_leases(sessions, |id| {
            (id == "leased").then(|| {
                json!({
                    "endpoint_id": "qwen-8080",
                    "model": "qwen-coder",
                    "state": "active",
                })
            })
        });

        assert_eq!(projected[0]["lease"]["endpoint_id"], "qwen-8080");
        assert!(projected[1].get("lease").is_none());
    }
}
