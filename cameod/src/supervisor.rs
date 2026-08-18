//! The endpoint supervisor: the daemon's record of every model server it has
//! started, and the one place that owns their live child processes.
//!
//! Planning stays in `cameo_placement`; this module never decides *how* to run a
//! model, only tracks the process once [`crate::app`] has built the command. It
//! leans on the execution boundary's non-blocking [`cameo_placement::spawn`],
//! which ties each child to the daemon's lifetime (`PR_SET_PDEATHSIG`) so a
//! crashed `cameod` never leaks a `llama-server` still holding VRAM.
//!
//! State is a `Mutex<HashMap>`: a control plane supervises a handful of
//! endpoints, so a single lock is simpler than anything finer-grained and never
//! a bottleneck. Every read reaps first (see [`Endpoint::refresh`]), so a server
//! that died on its own is reported as `exited`, not falsely `running`.

use std::collections::HashMap;
use std::process::Child;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use cameo_placement::{gib, spawn, CommandSpec};
use serde_json::{json, Value};

/// A crashed server (they do not exit cleanly) is restarted automatically, up to
/// this many times before it is parked as `failed` with the reason — so a broken
/// command flags itself instead of flapping forever.
const MAX_RESTARTS: u32 = 5;
/// Minimum gap between restart attempts, so a fast crash-loop backs off instead
/// of respawning on every dashboard poll.
const RESTART_BACKOFF: Duration = Duration::from_secs(2);

/// What to do with an endpoint whose child process is gone. Kept as a pure
/// decision so the timing and counting are unit-tested without spawning.
#[derive(Debug, PartialEq)]
enum Restart {
    /// Running, or already parked as failed — nothing to do.
    NotApplicable,
    /// Exited recently; wait for the backoff before retrying.
    Backoff,
    /// Exited, budget remains, backoff elapsed — respawn.
    Attempt,
    /// Exhausted the restart budget — park as failed.
    Exhausted,
}

fn restart_decision(
    running: bool,
    parked: bool,
    since_exit: Option<Duration>,
    restarts: u32,
) -> Restart {
    if running || parked {
        return Restart::NotApplicable;
    }
    let Some(elapsed) = since_exit else {
        return Restart::NotApplicable;
    };
    if elapsed < RESTART_BACKOFF {
        Restart::Backoff
    } else if restarts >= MAX_RESTARTS {
        Restart::Exhausted
    } else {
        Restart::Attempt
    }
}

/// A currently-resident endpoint, as the admission decision sees it: how much
/// VRAM it holds and when it was last used (for LRU eviction).
struct ResidentVram {
    id: String,
    vram_bytes: u64,
    last_used: SystemTime,
}

/// The outcome of admitting a new endpoint under the VRAM budget (F10).
#[derive(Debug, PartialEq)]
enum Admission {
    /// Fits in the remaining budget — start without disturbing anything.
    Admit,
    /// Fits only after stopping these endpoints (least-recently-used first).
    Evict(Vec<String>),
    /// Larger than the whole GPU — refuse even with nothing else resident.
    Refuse,
}

/// Decide admission for a `need`-byte endpoint against a known, non-zero VRAM
/// `budget`, given the endpoints already holding VRAM. Pure, so the arbitration
/// policy is unit-tested without spawning: a model bigger than the GPU is refused
/// (the planner's oversize case); otherwise the least-recently-used residents are
/// evicted until it fits. Callers with an *unknown* budget skip residency
/// entirely rather than pass `0` here — you cannot arbitrate what you cannot
/// measure.
fn admit(budget: u64, need: u64, residents: &mut [ResidentVram]) -> Admission {
    if need > budget {
        return Admission::Refuse;
    }
    let used: u64 = residents.iter().map(|r| r.vram_bytes).fold(0, u64::saturating_add);
    if used.saturating_add(need) <= budget {
        return Admission::Admit;
    }
    // Evict oldest-used first until the newcomer fits.
    residents.sort_by_key(|r| r.last_used);
    let must_free = used.saturating_add(need).saturating_sub(budget);
    let mut freed = 0u64;
    let mut evict = Vec::new();
    for r in residents.iter() {
        if freed >= must_free {
            break;
        }
        freed = freed.saturating_add(r.vram_bytes);
        evict.push(r.id.clone());
    }
    Admission::Evict(evict)
}

/// One supervised endpoint: what was asked for, the exact command, and — when it
/// spawned — the live process. The public view is produced by [`Endpoint::view`].
pub struct Endpoint {
    /// Stable identifier, `"<model-slug>-<port>"`, used in the URL path.
    pub id: String,
    /// The model name/alias/path as submitted.
    pub model: String,
    pub host: String,
    pub port: u16,
    /// Resolved backend label (`"Vulkan"` / `"Rocm"`), for display.
    pub backend: String,
    /// Whether the plan fit entirely in VRAM (a header stat, not a gate).
    pub fits_vram: bool,
    /// Human-readable plan notes, surfaced verbatim in the dashboard.
    pub notes: Vec<String>,
    /// The exact command that was (or would be) run.
    pub command: CommandSpec,
    /// The live child, once spawned. `None` before spawn, after reap, or when the
    /// spawn itself failed (see `error`).
    child: Option<Child>,
    /// Set when the spawn call itself failed (e.g. no `llama-server` on PATH, or
    /// a non-Linux dev host). Distinct from a process that ran and then exited.
    error: Option<String>,
    /// Exit code, set once the child has been reaped.
    exit_code: Option<i32>,
    started_at: SystemTime,
    /// How many times this endpoint has been auto-restarted after a crash.
    restarts: u32,
    /// When the current child last exited on its own; drives the restart backoff.
    last_exit_at: Option<SystemTime>,
    /// Estimated GPU VRAM this endpoint holds while running, for residency
    /// arbitration (F10). `0` when VRAM is unknown (residency then off).
    vram_bytes: u64,
    /// When this endpoint last served (or started). Drives LRU eviction; bumped by
    /// [`Supervisor::touch`] when the gateway routes a request to it.
    last_used: SystemTime,
}

impl Endpoint {
    /// Reap the child without blocking: if it has exited on its own, record the
    /// code and drop the handle. Idempotent, and the first thing every read does.
    fn refresh(&mut self) {
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.exit_code = status.code();
                    self.child = None;
                    self.last_exit_at = Some(SystemTime::now());
                }
                Ok(None) => {}
                Err(e) => {
                    self.error = Some(format!("wait failed: {e}"));
                    self.child = None;
                }
            }
        }
        self.maybe_restart();
    }

    /// Auto-restart a server that exited on its own. Reads drive this (the
    /// dashboard polls), the backoff keeps a crash-loop from respawning on every
    /// poll, and the cap turns a permanently-broken command into a `failed`
    /// endpoint that shows why — rather than flapping forever. A `stop()`ped
    /// endpoint is removed from the map, so only genuine crashes reach here.
    fn maybe_restart(&mut self) {
        let since = self.last_exit_at.and_then(|t| t.elapsed().ok());
        match restart_decision(
            self.child.is_some(),
            self.error.is_some(),
            since,
            self.restarts,
        ) {
            Restart::Attempt => match spawn(&self.command) {
                Ok(child) => {
                    self.child = Some(child);
                    self.restarts += 1;
                    self.exit_code = None;
                    self.last_exit_at = None;
                    self.started_at = SystemTime::now();
                }
                Err(e) => self.error = Some(format!("restart failed: {e}")),
            },
            Restart::Exhausted => {
                self.error = Some(format!(
                    "exited after {} restarts (last code {:?}); giving up",
                    self.restarts, self.exit_code
                ));
            }
            Restart::Backoff | Restart::NotApplicable => {}
        }
    }

    /// The lifecycle state, derived from what we know after a reap.
    fn state(&self) -> &'static str {
        if self.error.is_some() {
            "failed"
        } else if self.child.is_some() {
            "running"
        } else {
            "exited"
        }
    }

    /// The JSON the dashboard renders. Takes `&mut self` because it reaps first,
    /// so the reported state is never stale.
    fn view(&mut self) -> Value {
        self.refresh();
        let uptime = self.started_at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
        json!({
            "id": self.id,
            "model": self.model,
            "host": self.host,
            "port": self.port,
            "endpoint": format!("http://{}:{}", self.host, self.port),
            "backend": self.backend,
            "state": self.state(),
            "pid": self.child.as_ref().map(Child::id),
            "exit_code": self.exit_code,
            "error": self.error,
            "restarts": self.restarts,
            "fits_vram": self.fits_vram,
            "vram_bytes": self.vram_bytes,
            "notes": self.notes,
            "command": self.command.display(),
            "uptime_secs": uptime,
        })
    }
}

/// Everything [`crate::app`] must hand the supervisor to start an endpoint: the
/// planning result already reduced to display facts, plus the command to run.
pub struct StartRequest {
    pub model: String,
    pub host: String,
    pub port: u16,
    pub backend: String,
    pub fits_vram: bool,
    pub notes: Vec<String>,
    pub command: CommandSpec,
    /// Estimated VRAM the endpoint will hold, for residency (F10).
    pub vram_need: u64,
    /// The box's usable VRAM budget. `0` = unknown → residency is skipped.
    pub vram_budget: u64,
}

/// Why a start was refused before any process was spawned.
#[derive(Debug)]
pub enum StartError {
    /// An endpoint with this id is already tracked and still running.
    PortInUse(String),
    /// The model is larger than the whole GPU — refused rather than OOM (F10).
    WontFit(String),
}

/// The supervisor: a lock around the set of tracked endpoints.
#[derive(Default)]
pub struct Supervisor {
    endpoints: Mutex<HashMap<String, Endpoint>>,
}

impl Supervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start (or record the failure of starting) an endpoint. On a spawn error
    /// the endpoint is still stored in the `failed` state and its view returned,
    /// so the dashboard shows *why* it did not come up rather than nothing.
    pub fn start(&self, req: StartRequest) -> Result<Value, StartError> {
        let id = endpoint_id(&req.model, req.port);
        let mut map = self.endpoints.lock().unwrap();

        // Reap any prior tenant of this id before deciding the port is taken: a
        // crashed endpoint should not block re-launching on the same port.
        if let Some(existing) = map.get_mut(&id) {
            existing.refresh();
            if existing.state() == "running" {
                return Err(StartError::PortInUse(id));
            }
        }

        // Residency admission (F10): only when we actually know the VRAM budget.
        // Reap first so a crashed endpoint is not counted as holding VRAM, then
        // arbitrate — evicting least-recently-used residents or refusing outright.
        if req.vram_budget > 0 && req.vram_need > 0 {
            let mut residents: Vec<ResidentVram> = map
                .values_mut()
                .filter_map(|e| {
                    e.refresh();
                    (e.id != id && e.state() == "running" && e.vram_bytes > 0).then(|| {
                        ResidentVram {
                            id: e.id.clone(),
                            vram_bytes: e.vram_bytes,
                            last_used: e.last_used,
                        }
                    })
                })
                .collect();
            match admit(req.vram_budget, req.vram_need, &mut residents) {
                Admission::Admit => {}
                Admission::Evict(ids) => {
                    for victim in ids {
                        if let Some(mut e) = map.remove(&victim) {
                            if let Some(mut child) = e.child.take() {
                                let _ = child.kill();
                                let _ = child.wait();
                            }
                        }
                    }
                }
                Admission::Refuse => {
                    return Err(StartError::WontFit(format!(
                        "model needs ~{:.1} GiB of VRAM but the GPU has ~{:.1} GiB; \
                         quantize further, pick a smaller model, or add a GPU.",
                        gib(req.vram_need),
                        gib(req.vram_budget),
                    )));
                }
            }
        }

        let (child, error) = match spawn(&req.command) {
            Ok(child) => (Some(child), None),
            Err(e) => (None, Some(e.to_string())),
        };

        let mut endpoint = Endpoint {
            id: id.clone(),
            model: req.model,
            host: req.host,
            port: req.port,
            backend: req.backend,
            fits_vram: req.fits_vram,
            notes: req.notes,
            command: req.command,
            child,
            error,
            exit_code: None,
            started_at: SystemTime::now(),
            restarts: 0,
            last_exit_at: None,
            vram_bytes: req.vram_need,
            last_used: SystemTime::now(),
        };
        let view = endpoint.view();
        map.insert(id, endpoint);
        Ok(view)
    }

    /// Mark an endpoint as just-used, so LRU eviction (F10) reflects real traffic.
    /// Called by the gateway (F8) when it routes a request to this endpoint.
    pub fn touch(&self, id: &str) {
        if let Some(e) = self.endpoints.lock().unwrap().get_mut(id) {
            e.last_used = SystemTime::now();
        }
    }

    /// The `(host, port, id)` of a running endpoint serving `model`, for the F8
    /// gateway to proxy to. Reaps first so a crashed endpoint is not routed to.
    pub fn endpoint_for_model(&self, model: &str) -> Option<(String, u16, String)> {
        let mut map = self.endpoints.lock().unwrap();
        map.values_mut().find_map(|e| {
            e.refresh();
            (e.model == model && e.state() == "running")
                .then(|| (e.host.clone(), e.port, e.id.clone()))
        })
    }

    /// Distinct model names currently served (running), for `GET /v1/models`.
    pub fn served_models(&self) -> Vec<String> {
        let mut map = self.endpoints.lock().unwrap();
        let mut names: Vec<String> = map
            .values_mut()
            .filter_map(|e| {
                e.refresh();
                (e.state() == "running").then(|| e.model.clone())
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// The current view of every tracked endpoint, most-recently-started first.
    pub fn list(&self) -> Vec<Value> {
        let mut map = self.endpoints.lock().unwrap();
        let mut views: Vec<(SystemTime, Value)> =
            map.values_mut().map(|e| (e.started_at, e.view())).collect();
        views.sort_by_key(|v| std::cmp::Reverse(v.0));
        views.into_iter().map(|(_, v)| v).collect()
    }

    /// One endpoint's view by id, or `None` if unknown.
    pub fn get(&self, id: &str) -> Option<Value> {
        let mut map = self.endpoints.lock().unwrap();
        map.get_mut(id).map(Endpoint::view)
    }

    /// Stop and forget an endpoint. Returns `false` if the id is unknown. Killing
    /// a child that already exited is harmless; we ignore that error and still
    /// drop the record.
    pub fn stop(&self, id: &str) -> bool {
        let mut map = self.endpoints.lock().unwrap();
        match map.remove(id) {
            Some(mut endpoint) => {
                if let Some(mut child) = endpoint.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                true
            }
            None => false,
        }
    }

    /// The endpoint half of `/metrics`, in Prometheus text exposition format
    /// (F11). Reaps first (so `up` and uptime are current), then emits each metric
    /// family with its `HELP`/`TYPE` header followed by all its samples — the
    /// order Prometheus requires. GPU-level metrics are appended by the caller,
    /// which owns detection.
    pub fn metrics(&self) -> String {
        let mut map = self.endpoints.lock().unwrap();

        let mut up = String::new();
        let mut restarts = String::new();
        let mut uptime = String::new();
        let mut vram = String::new();
        for e in map.values_mut() {
            e.refresh();
            let running = if e.state() == "running" { 1 } else { 0 };
            let labels = format!(
                "id=\"{}\",model=\"{}\",port=\"{}\",backend=\"{}\",state=\"{}\"",
                esc(&e.id),
                esc(&e.model),
                e.port,
                esc(&e.backend),
                e.state()
            );
            up.push_str(&format!("cameo_endpoint_up{{{labels}}} {running}\n"));
            let id_label = format!("id=\"{}\"", esc(&e.id));
            restarts.push_str(&format!(
                "cameo_endpoint_restarts_total{{{id_label}}} {}\n",
                e.restarts
            ));
            let secs = e.started_at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
            uptime.push_str(&format!(
                "cameo_endpoint_uptime_seconds{{{id_label}}} {secs}\n"
            ));
            vram.push_str(&format!(
                "cameo_endpoint_vram_bytes{{{id_label}}} {}\n",
                e.vram_bytes
            ));
        }

        let mut out = String::new();
        out.push_str("# HELP cameo_up 1 if the control-plane daemon is serving.\n");
        out.push_str("# TYPE cameo_up gauge\ncameo_up 1\n");
        out.push_str("# HELP cameo_endpoints Number of tracked model endpoints.\n");
        out.push_str(&format!(
            "# TYPE cameo_endpoints gauge\ncameo_endpoints {}\n",
            map.len()
        ));
        out.push_str("# HELP cameo_endpoint_up 1 if the endpoint's process is running.\n");
        out.push_str("# TYPE cameo_endpoint_up gauge\n");
        out.push_str(&up);
        out.push_str("# HELP cameo_endpoint_restarts_total Auto-restarts since creation.\n");
        out.push_str("# TYPE cameo_endpoint_restarts_total counter\n");
        out.push_str(&restarts);
        out.push_str("# HELP cameo_endpoint_uptime_seconds Seconds since the current process started.\n");
        out.push_str("# TYPE cameo_endpoint_uptime_seconds gauge\n");
        out.push_str(&uptime);
        out.push_str("# HELP cameo_endpoint_vram_bytes Estimated VRAM the endpoint holds.\n");
        out.push_str("# TYPE cameo_endpoint_vram_bytes gauge\n");
        out.push_str(&vram);
        out
    }
}

/// Escape a Prometheus label value: backslash, double-quote, and newline are the
/// only characters the exposition format requires escaping. Model ids can be
/// arbitrary paths, so this is not optional. Shared with [`crate::app`]'s GPU
/// metrics so both escape identically.
pub(crate) fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

/// Derive the stable endpoint id from a model name and port. The port makes it
/// unique on one host (two servers cannot bind the same port), and the slug
/// keeps it readable in the URL and the UI.
fn endpoint_id(model: &str, port: u16) -> String {
    let slug: String = model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    // Collapse the runs of '-' a path or extension leaves behind.
    let slug = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "model" } else { &slug };
    format!("{slug}-{port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> CommandSpec {
        CommandSpec {
            program: "llama-server".into(),
            args: vec!["-m".into(), "/m.gguf".into()],
            env: Vec::new(),
        }
    }

    fn req(model: &str, port: u16) -> StartRequest {
        StartRequest {
            model: model.into(),
            host: "127.0.0.1".into(),
            port,
            backend: "Vulkan".into(),
            fits_vram: true,
            notes: vec![],
            command: spec(),
            vram_need: 0,
            vram_budget: 0,
        }
    }

    fn resident(id: &str, vram: u64, age_secs: u64) -> ResidentVram {
        ResidentVram {
            id: id.into(),
            vram_bytes: vram,
            last_used: SystemTime::now() - Duration::from_secs(age_secs),
        }
    }

    #[test]
    fn admit_when_it_fits_without_eviction() {
        let mut r = vec![resident("a", 4, 10)];
        assert_eq!(admit(16, 8, &mut r), Admission::Admit);
    }

    #[test]
    fn refuse_when_larger_than_the_whole_gpu() {
        let mut r = vec![];
        assert_eq!(admit(16, 20, &mut r), Admission::Refuse);
    }

    #[test]
    fn evict_least_recently_used_until_it_fits() {
        // Budget 16; residents hold 12 (a=oldest, c=newest); newcomer needs 8, so
        // 4 must be freed — the single oldest (a=6) suffices, and only it goes.
        let mut r = vec![
            resident("a", 6, 100), // oldest
            resident("b", 4, 50),
            resident("c", 2, 10), // newest
        ];
        assert_eq!(admit(16, 8, &mut r), Admission::Evict(vec!["a".into()]));
    }

    #[test]
    fn evict_spills_to_the_next_lru_when_one_is_not_enough() {
        // Need 14 into a 16 budget with 12 resident → free 10; a(6)+b(4)=10.
        let mut r = vec![
            resident("a", 6, 100),
            resident("b", 4, 50),
            resident("c", 2, 10),
        ];
        assert_eq!(
            admit(16, 14, &mut r),
            Admission::Evict(vec!["a".into(), "b".into()])
        );
    }

    #[test]
    fn id_slugs_paths_and_appends_port() {
        assert_eq!(endpoint_id("tinyllama", 8080), "tinyllama-8080");
        assert_eq!(
            endpoint_id("/models/My Models/llama 7b.gguf", 9000),
            "models-my-models-llama-7b-gguf-9000"
        );
        assert_eq!(endpoint_id("", 1), "model-1");
    }

    #[test]
    fn start_records_a_failed_spawn_rather_than_dropping_it() {
        // On this dev host the execution boundary refuses to spawn, so the
        // endpoint lands in `failed` — and, crucially, is still listed with the
        // reason attached. That is the behaviour the dashboard depends on.
        let sup = Supervisor::new();
        let view = sup.start(req("tinyllama", 8080)).unwrap();
        assert_eq!(view["id"], "tinyllama-8080");
        assert_eq!(view["state"], "failed");
        assert!(view["error"].is_string());

        assert_eq!(sup.list().len(), 1);
        assert!(sup.get("tinyllama-8080").is_some());
    }

    #[test]
    fn stop_forgets_the_endpoint_and_reports_unknown_ids() {
        let sup = Supervisor::new();
        sup.start(req("tinyllama", 8080)).unwrap();
        assert!(sup.stop("tinyllama-8080"));
        assert!(sup.get("tinyllama-8080").is_none());
        assert!(!sup.stop("tinyllama-8080"));
    }

    #[test]
    fn a_failed_endpoint_does_not_block_relaunch_on_its_port() {
        // The first start failed (dev host), so it is not "running"; starting the
        // same model+port again must be allowed, not rejected as PortInUse.
        let sup = Supervisor::new();
        sup.start(req("tinyllama", 8080)).unwrap();
        assert!(sup.start(req("tinyllama", 8080)).is_ok());
    }

    #[test]
    fn metrics_emit_prometheus_families_for_each_endpoint() {
        let sup = Supervisor::new();
        sup.start(req("tinyllama", 8080)).unwrap();
        let m = sup.metrics();
        // Family headers present exactly once.
        assert_eq!(m.matches("# TYPE cameo_endpoint_up gauge").count(), 1);
        assert!(m.contains("cameo_up 1"));
        assert!(m.contains("cameo_endpoints 1"));
        // The endpoint sample carries its identifying labels.
        assert!(m.contains(r#"cameo_endpoint_up{id="tinyllama-8080",model="tinyllama",port="8080""#));
        assert!(m.contains(r#"cameo_endpoint_restarts_total{id="tinyllama-8080"} 0"#));
    }

    #[test]
    fn label_values_are_escaped() {
        assert_eq!(esc(r#"a"b\c"#), r#"a\"b\\c"#);
        assert_eq!(esc("line\nbreak"), "line\\nbreak");
        assert_eq!(esc("plain"), "plain");
    }

    #[test]
    fn restart_decision_covers_the_states() {
        // Running or already parked → leave alone.
        assert_eq!(
            restart_decision(true, false, None, 0),
            Restart::NotApplicable
        );
        assert_eq!(
            restart_decision(false, true, Some(Duration::from_secs(10)), 0),
            Restart::NotApplicable
        );
        // Exited but no timestamp yet → nothing to act on.
        assert_eq!(
            restart_decision(false, false, None, 0),
            Restart::NotApplicable
        );
        // Exited recently → back off.
        assert_eq!(
            restart_decision(false, false, Some(Duration::from_millis(100)), 0),
            Restart::Backoff
        );
        // Backoff elapsed, budget left → attempt.
        assert_eq!(
            restart_decision(false, false, Some(Duration::from_secs(5)), 2),
            Restart::Attempt
        );
        // Budget exhausted → give up.
        assert_eq!(
            restart_decision(false, false, Some(Duration::from_secs(5)), MAX_RESTARTS),
            Restart::Exhausted
        );
    }
}
