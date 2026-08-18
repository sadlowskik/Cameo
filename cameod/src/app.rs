//! The control-plane application: request routing and the glue that reuses
//! Cameo's detection and placement brain to answer the dashboard's API.
//!
//! Everything here is the same logic the `cameo` CLI runs — detect the topology,
//! classify tiers, plan a placement, build a `llama-server` command — exposed
//! over HTTP instead of a terminal. The daemon adds exactly one capability the
//! CLI lacks: it *keeps* the spawned process, via [`crate::supervisor`]. The
//! HTTP plumbing lives in [`crate::http`]; this module only decides what each
//! route means.

use std::sync::Arc;

use cameo_config::{Backend, Settings};
use cameo_gpu_detect::{classify_topology, detect_topology_or_cpu, Captures, OverrideDb};
use cameo_placement::command::build_llama_server;
use cameo_placement::{plan as make_plan, ModelMeta, QuantLevel, Task};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::http::{Request, Response};
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
    /// If set, every `/api/*` request must present this as a bearer token. The
    /// dashboard at `/` is always reachable so it can prompt for the key.
    pub console_key: Option<String>,
}

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
    #[serde(default = "default_params")]
    params: f64,
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
fn default_params() -> f64 {
    7.0
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
        let mut m = if self.moe {
            ModelMeta::moe(&self.model, self.params, quant)
        } else {
            ModelMeta::dense(&self.model, self.params, quant)
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
            [] => return Response::html(crate::dashboard::INDEX_HTML),
            ["healthz"] => return Response::json(200, &json!({ "status": "ok" })),
            ["readyz"] => {
                // Ready = the node can actually detect hardware and plan work.
                return match detect_report(state) {
                    Ok(_) => Response::json(200, &json!({ "ready": true })),
                    Err(_) => Response::json(503, &json!({ "ready": false })),
                };
            }
            // Prometheus scrape (F11). Unauthenticated like the probes so any
            // scraper reaches it; the console reads the same endpoint for tiles.
            ["metrics"] => return metrics_response(state),
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

    // Everything under /api is gated by the console key, when one is configured.
    if segs.first() == Some(&"api") {
        if let Some(denied) = check_auth(state, req) {
            return denied;
        }
        return route_api(state, req, &segs[1..]).no_store();
    }

    Response::error(404, "not found")
}

/// Authenticate a `/v1` request against the serve key. `None` = allowed. No serve
/// key configured means the gateway is open (loopback dev), mirroring the
/// per-endpoint serving rule.
fn check_serve_auth(state: &Arc<AppState>, req: &Request) -> Option<Response> {
    let key = state.settings.serve_api_key.as_deref()?;
    let presented = req
        .header("authorization")
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::trim);
    if presented == Some(key) {
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
    let model = serde_json::from_slice::<Value>(&req.body)
        .ok()
        .and_then(|v| {
            v.get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let Some(model) = model else {
        return Response::error(400, "request body must be JSON with a \"model\" field");
    };

    let Some((host, port, id)) = state.sup.endpoint_for_model(&model) else {
        return Response::error(
            404,
            format!("no running endpoint serves model '{model}'. Start one via POST /api/servers."),
        );
    };
    state.sup.touch(&id);

    let content_type = req.header("content-type").unwrap_or("application/json");
    match crate::proxy::forward(
        &host,
        port,
        &req.method,
        &req.path,
        content_type,
        &req.body,
        state.settings.serve_api_key.as_deref(),
    ) {
        Ok(b) => Response::new(b.status, &b.content_type, b.body),
        Err(e) => Response::error(502, format!("upstream {host}:{port} unreachable: {e}")),
    }
}

/// Authenticate an `/api` request. `None` means allowed; `Some(resp)` is the
/// `401` to return. No configured key means the console is open (loopback dev).
fn check_auth(state: &Arc<AppState>, req: &Request) -> Option<Response> {
    let key = state.console_key.as_deref()?;
    let presented = req
        .header("authorization")
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::trim);
    if presented == Some(key) {
        None
    } else {
        Some(Response::error(401, "missing or invalid console key"))
    }
}

fn route_api(state: &Arc<AppState>, req: &Request, rest: &[&str]) -> Response {
    match (req.method.as_str(), rest) {
        ("GET", ["gpus"]) => api_gpus(state),
        ("GET", ["models"]) => api_models(),
        ("POST", ["plan"]) => api_plan(state, req),
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

// ---- detection -------------------------------------------------------------

/// Detect + classify, mapping detection errors to an HTTP response. `Ok` carries
/// the JSON GPU report the dashboard renders.
fn detect_report(state: &Arc<AppState>) -> Result<Value, Response> {
    let topo = detect_topology_or_cpu(&state.captures).map_err(|e| match e {
        cameo_gpu_detect::Error::UnsupportedOs => Response::error(
            501,
            "live GPU detection needs Linux. Start cameod with captured fixtures \
             (--lspci-file, …) to drive the console on this host.",
        ),
        cameo_gpu_detect::Error::NoGpu => Response::error(404, "no AMD GPU detected"),
        other => Response::error(500, other.to_string()),
    })?;
    let assessments = classify_topology(&topo, &OverrideDb::embedded());

    Ok(json!({
        "gpus": assessments,
        "host_mem": topo.host_mem,
        "links": topo.links.iter().map(|l| json!({
            "a": l.a, "b": l.b, "kind": format!("{:?}", l.kind),
        })).collect::<Vec<_>>(),
        "bottleneck": topo.bottleneck_link().map(|k| format!("{k:?}")),
    }))
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

    if let Ok(topo) = detect_topology_or_cpu(&state.captures) {
        let assessments = classify_topology(&topo, &OverrideDb::embedded());
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

fn api_models() -> Response {
    let aliases: Vec<Value> = cameo_models::aliases()
        .into_iter()
        .map(|a| json!({ "name": a.name, "repo": a.repo, "file": a.file }))
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
    let topo = detect_topology_or_cpu(&state.captures).map_err(|e| match e {
        cameo_gpu_detect::Error::UnsupportedOs => Response::error(
            501,
            "live GPU detection needs Linux. Start cameod with captured fixtures to plan here.",
        ),
        cameo_gpu_detect::Error::NoGpu => Response::error(404, "no AMD GPU detected"),
        other => Response::error(500, other.to_string()),
    })?;
    let assessments = classify_topology(&topo, &OverrideDb::embedded());

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
