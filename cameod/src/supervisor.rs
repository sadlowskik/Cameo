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
use std::time::SystemTime;

use cameo_placement::{spawn, CommandSpec};
use serde_json::{json, Value};

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
                }
                Ok(None) => {}
                Err(e) => {
                    self.error = Some(format!("wait failed: {e}"));
                    self.child = None;
                }
            }
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
            "fits_vram": self.fits_vram,
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
}

/// Why a start was refused before any process was spawned.
#[derive(Debug)]
pub enum StartError {
    /// An endpoint with this id is already tracked and still running.
    PortInUse(String),
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
        };
        let view = endpoint.view();
        map.insert(id, endpoint);
        Ok(view)
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
        }
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
}
