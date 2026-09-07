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

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
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
/// A child that served at least this long before exiting was working, not
/// crash-looping — its exit resets the restart budget. Without this the cap is
/// lifetime: a server that hiccups once every few days eventually exhausts its
/// five restarts and is parked `failed` for good, which punishes exactly the
/// endpoints that were healthy.
const STABLE_UPTIME_RESET: Duration = Duration::from_secs(300);
const HEALTH_PROBE_INTERVAL: Duration = Duration::from_millis(500);
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(250);
const LEASE_TTL_SECS: u64 = 90;

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

trait ManagedProcess {
    fn exited(&mut self) -> std::io::Result<bool>;
    fn terminate(&mut self) -> std::io::Result<()>;
}

impl ManagedProcess for Child {
    fn exited(&mut self) -> std::io::Result<bool> {
        self.try_wait().map(|status| status.is_some())
    }
    fn terminate(&mut self) -> std::io::Result<()> {
        self.kill()
    }
}

fn confirm_termination(child: &mut impl ManagedProcess, timeout: Duration) -> Result<(), String> {
    if child
        .exited()
        .map_err(|e| format!("cannot observe owned process: {e}"))?
    {
        return Ok(());
    }
    if let Err(error) = child.terminate() {
        if child.exited().unwrap_or(false) {
            return Ok(());
        }
        return Err(format!("cannot terminate owned process: {error}"));
    }
    let started = std::time::Instant::now();
    loop {
        if child
            .exited()
            .map_err(|e| format!("cannot confirm process exit: {e}"))?
        {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            return Err("owned process has not exited before the stop deadline".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

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

/// The restart budget carried forward after a child exits: an exit after a
/// stable stretch of service wipes the slate, a quick crash keeps the count.
/// Pure, so the windowing policy is unit-tested without clocks or spawning.
fn restarts_after_exit(restarts: u32, uptime: Duration) -> u32 {
    if uptime >= STABLE_UPTIME_RESET {
        0
    } else {
        restarts
    }
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
    let used: u64 = residents
        .iter()
        .map(|r| r.vram_bytes)
        .fold(0, u64::saturating_add);
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
    restart_enabled: bool,
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
    /// Context window requested when the server was started. This is a server
    /// fact, not the model's advertised maximum.
    pub context_tokens: u32,
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
    /// A live process is only `starting`; it becomes routable after llama.cpp's
    /// `/health` endpoint answers successfully.
    ready: bool,
    last_health_probe: Option<SystemTime>,
    readiness_error: Option<String>,
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
                    // A long, healthy run earns back the restart budget.
                    let uptime = self.started_at.elapsed().unwrap_or(Duration::ZERO);
                    self.restarts = restarts_after_exit(self.restarts, uptime);
                }
                Ok(None) => {}
                Err(e) => {
                    self.error = Some(format!("wait failed: {e}"));
                    self.ready = false;
                }
            }
        }
        if self.child.is_some()
            && self
                .last_health_probe
                .and_then(|at| at.elapsed().ok())
                .map(|age| age >= HEALTH_PROBE_INTERVAL)
                .unwrap_or(true)
        {
            self.last_health_probe = Some(SystemTime::now());
            match probe_health(&self.host, self.port) {
                Ok(()) => {
                    self.ready = true;
                    self.readiness_error = None;
                }
                Err(error) => {
                    self.ready = false;
                    self.readiness_error = Some(error);
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
        if !self.restart_enabled {
            return;
        }
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
                    self.ready = false;
                    self.last_health_probe = None;
                    self.readiness_error = None;
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

    fn stop_owned(&mut self) -> Result<(), String> {
        self.error = Some("stop requested; automatic restart disabled".into());
        self.ready = false;
        if let Some(child) = self.child.as_mut() {
            if let Err(error) = confirm_termination(child, Duration::from_secs(1)) {
                self.error = Some(error.clone());
                return Err(error);
            }
        }
        self.child = None;
        self.last_exit_at = None;
        Ok(())
    }

    /// The lifecycle state, derived from what we know after a reap.
    fn state(&self) -> &'static str {
        if self.error.is_some() {
            "failed"
        } else if self.child.is_some() && self.ready {
            "running"
        } else if self.child.is_some() {
            "starting"
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
            "readiness_error": self.readiness_error,
            "restarts": self.restarts,
            "fits_vram": self.fits_vram,
            "vram_bytes": self.vram_bytes,
            "notes": self.notes,
            "context_tokens": self.context_tokens,
            "command": self.command.display(),
            "uptime_secs": uptime,
        })
    }

    fn process_running(&self) -> bool {
        self.child.is_some()
    }

    fn is_ready(&self) -> bool {
        self.child.is_some() && self.ready && self.error.is_none()
    }
}

fn probe_health(host: &str, port: u16) -> Result<(), String> {
    let connect_host = match host {
        "0.0.0.0" => "127.0.0.1",
        "::" => "::1",
        other => other,
    };
    let addr = (connect_host, port)
        .to_socket_addrs()
        .map_err(|e| format!("health address: {e}"))?
        .next()
        .ok_or_else(|| "health address did not resolve".to_string())?;
    let mut stream = TcpStream::connect_timeout(&addr, HEALTH_PROBE_TIMEOUT)
        .map_err(|e| format!("health connect: {e}"))?;
    stream
        .set_read_timeout(Some(HEALTH_PROBE_TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(HEALTH_PROBE_TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(|e| format!("health write: {e}"))?;
    let mut status = String::new();
    BufReader::new(stream)
        .read_line(&mut status)
        .map_err(|e| format!("health read: {e}"))?;
    let code = status
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| format!("invalid health response: {}", status.trim()))?;
    if (200..300).contains(&code) {
        Ok(())
    } else {
        Err(format!("health returned HTTP {code}"))
    }
}

/// Everything [`crate::app`] must hand the supervisor to start an endpoint: the
/// planning result already reduced to display facts, plus the command to run.
pub struct StartRequest {
    /// Validated high-level API configuration, without commands or credentials.
    pub intent: Value,
    pub model: String,
    pub host: String,
    pub port: u16,
    pub backend: String,
    pub fits_vram: bool,
    pub notes: Vec<String>,
    /// Requested server context window, in tokens.
    pub context_tokens: u32,
    pub command: CommandSpec,
    /// Estimated VRAM the endpoint will hold, for residency (F10).
    pub vram_need: u64,
    /// The box's usable VRAM budget. `0` = unknown → residency is skipped.
    pub vram_budget: u64,
    /// Normal operator starts may reuse unleased capacity.  Knossos session
    /// starts set this false until an operator has reviewed the named victims.
    pub allow_evict: bool,
}

/// Why a start was refused before any process was spawned.
#[derive(Debug)]
pub enum StartError {
    ShuttingDown,
    Termination(String),
    Persistence(String),
    /// An endpoint with this id is already tracked and still running.
    PortInUse(String),
    /// The model is larger than the whole GPU — refused rather than OOM (F10).
    WontFit(String),
    /// The model would fit only by evicting an endpoint an active session has
    /// explicitly claimed. The operator may release the lease or stop it.
    LeasedCapacity(String),
    /// The model fits only if these unleased endpoints are stopped.  Returned
    /// instead of silently evicting when a mission has not been approved for it.
    EvictionRequired(Vec<String>),
}

/// Why an explicit session lease could not be created.
#[derive(Debug)]
pub enum LeaseError {
    Persistence(String),
    /// No currently running endpoint serves the requested model.
    Unavailable(String),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Lease {
    session_id: String,
    model: String,
    endpoint_id: String,
    #[serde(skip)]
    recovered: bool,
    #[serde(default)]
    expires_at: u64,
    #[serde(skip)]
    renewed_at: Option<std::time::Instant>,
}

impl Lease {
    fn expired(&self, now: u64) -> bool {
        self.expires_at <= now
            || self
                .renewed_at
                .is_some_and(|at| at.elapsed().as_secs() >= LEASE_TTL_SECS)
    }
}

/// The supervisor: a lock around the set of tracked endpoints.
#[derive(Default)]
pub struct Supervisor {
    shutting_down: std::sync::atomic::AtomicBool,
    endpoints: Mutex<HashMap<String, Endpoint>>,
    /// Session-id to the endpoint it explicitly claims. Kept separately from
    /// endpoint state so an endpoint that stops can report `unavailable`
    /// rather than silently forgetting the session's claim.
    leases: Mutex<HashMap<String, Lease>>,
    store: Mutex<Option<crate::endpoint_store::EndpointStore>>,
}

impl Supervisor {
    pub fn begin_shutdown(&self) {
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
        for endpoint in self.endpoints.lock().unwrap().values_mut() {
            endpoint.restart_enabled = false;
        }
    }

    /// Stop owned children without erasing desired endpoint state for the next boot.
    pub fn shutdown_owned(&self) -> Result<(), String> {
        self.begin_shutdown();
        let mut map = self.endpoints.lock().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let mut failed = 0;
        for endpoint in map.values_mut() {
            if std::time::Instant::now() >= deadline || endpoint.stop_owned().is_err() {
                failed += 1;
            }
        }
        if failed == 0 {
            Ok(())
        } else {
            Err(format!("{failed} owned endpoints did not confirm shutdown"))
        }
    }
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let mut store = crate::endpoint_store::EndpointStore::open(path)?;
        let mut leases = HashMap::new();
        let now = epoch_seconds();
        let mut migrated = false;
        for (id, value) in &store.state.leases {
            let mut lease: Lease =
                serde_json::from_value(value.clone()).map_err(|_| "invalid persisted lease")?;
            if lease.session_id != *id
                || id.is_empty()
                || lease.endpoint_id.is_empty()
                || lease.model.is_empty()
            {
                return Err("invalid persisted lease identity".into());
            }
            lease.recovered = true;
            let bounded = if lease.expires_at == 0 {
                now.saturating_add(LEASE_TTL_SECS)
            } else {
                lease.expires_at.min(now.saturating_add(LEASE_TTL_SECS))
            };
            migrated |= lease.expires_at != bounded;
            lease.expires_at = bounded;
            lease.renewed_at = Some(std::time::Instant::now());
            leases.insert(id.clone(), lease);
        }
        if migrated {
            let mut next = store.state.clone();
            for (id, lease) in &leases {
                next.leases.insert(
                    id.clone(),
                    serde_json::to_value(lease).map_err(|e| e.to_string())?,
                );
            }
            store.commit(next)?;
        }
        Ok(Self {
            leases: Mutex::new(leases),
            store: Mutex::new(Some(store)),
            ..Self::default()
        })
    }

    fn persist_leases(&self, leases: &HashMap<String, Lease>) -> Result<(), String> {
        let mut storage = self.store.lock().unwrap();
        if let Some(store) = storage.as_mut() {
            let mut next = store.state.clone();
            next.leases = leases
                .iter()
                .map(|(id, lease)| serde_json::to_value(lease).map(|value| (id.clone(), value)))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?;
            store.commit(next)?;
        }
        Ok(())
    }

    pub fn recovery_intents(&self) -> Vec<(String, Value)> {
        self.store
            .lock()
            .unwrap()
            .as_ref()
            .map(|store| {
                store
                    .state
                    .endpoints
                    .iter()
                    .map(|(id, value)| (id.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn recovery_sessions(&self) -> Vec<Value> {
        self.store
            .lock()
            .unwrap()
            .as_ref()
            .map(|store| store.state.sessions.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn persist_session(&self, session: &crate::sessions::Session) -> Result<(), String> {
        let mut storage = self.store.lock().unwrap();
        if let Some(store) = storage.as_mut() {
            let mut next = store.state.clone();
            // Recovery identity only; mission text, files, paths and reported proof stay out.
            next.sessions.insert(session.id.clone(), json!({"id":session.id, "name":session.name,
                "role":session.role, "mode":session.mode, "engine":session.engine, "model":session.model}));
            store.commit(next)?;
        }
        Ok(())
    }

    pub fn remove_session(&self, id: &str) -> Result<(), String> {
        let mut leases = self.leases.lock().unwrap();
        let mut storage = self.store.lock().unwrap();
        if let Some(store) = storage.as_mut() {
            let mut next = store.state.clone();
            next.sessions.remove(id);
            next.leases.remove(id);
            store.commit(next)?;
        }
        leases.remove(id);
        Ok(())
    }

    pub fn has_history(&self) -> bool {
        self.store
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|store| store.has_history())
    }

    fn commit_intent(&self, remove: &[String], add: Option<(&str, &Value)>) -> Result<(), String> {
        let mut storage = self.store.lock().unwrap();
        if let Some(store) = storage.as_mut() {
            let mut next = store.state.clone();
            for id in remove {
                next.endpoints.remove(id);
            }
            if let Some((id, intent)) = add {
                next.endpoints.insert(id.into(), intent.clone());
            }
            store.commit(next)?;
        }
        Ok(())
    }

    /// Reap, restart, and probe all endpoints. Called by a daemon maintenance
    /// thread so lifecycle progress does not depend on dashboard traffic.
    pub fn maintain(&self) {
        if let Err(error) = self.expire_leases(epoch_seconds()) {
            tracing::error!(%error, "cannot persist expired lease release");
        }
        let mut endpoints = self.endpoints.lock().unwrap();
        for endpoint in endpoints.values_mut() {
            endpoint.refresh();
        }
        let mut storage = self.store.lock().unwrap();
        if let Some(store) = storage.as_mut() {
            let mut next = store.state.clone();
            let mut changed = false;
            for endpoint in endpoints.values() {
                if let Some(intent) = next.endpoints.get_mut(&endpoint.id) {
                    let observation = json!({"state": endpoint.state(), "restarts": endpoint.restarts, "ready": endpoint.is_ready()});
                    if intent.get("_last_observed") != Some(&observation) {
                        intent["_last_observed"] = observation;
                        changed = true;
                    }
                    if endpoint.is_ready()
                        && endpoint.started_at.elapsed().unwrap_or_default() >= STABLE_UPTIME_RESET
                        && intent
                            .get("_recovery_attempts")
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
                            > 0
                    {
                        intent["_recovery_attempts"] = json!(0);
                        changed = true;
                    }
                }
            }
            if changed {
                if let Err(error) = store.commit(next) {
                    tracing::error!(%error, "cannot persist supervisor observation");
                }
            }
        }
    }

    /// Start (or record the failure of starting) an endpoint. On a spawn error
    /// the endpoint is still stored in the `failed` state and its view returned,
    /// so the dashboard shows *why* it did not come up rather than nothing.
    pub fn start(&self, req: StartRequest) -> Result<Value, StartError> {
        let id = endpoint_id(&req.model, req.port);
        let mut map = self.endpoints.lock().unwrap();
        let leases = self.leases.lock().unwrap();
        let mut eviction_ids = Vec::new();
        if self.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(StartError::ShuttingDown);
        }

        // Reap any prior tenant of this id before deciding the port is taken: a
        // crashed endpoint should not block re-launching on the same port.
        if let Some(existing) = map.get_mut(&id) {
            existing.refresh();
            if existing.process_running() {
                return Err(StartError::PortInUse(id));
            }
        }

        // The id is `<model>-<port>`, so a *different* model on the same port has
        // a different id and slips past the check above — the newcomer would then
        // fail to bind and burn its restart budget with a generic crash message.
        // Refuse up front, naming the endpoint that holds the port.
        if let Some(holder) = map
            .values_mut()
            .filter(|e| e.id != id && e.port == req.port)
            .find_map(|e| {
                e.refresh();
                e.process_running().then(|| e.id.clone())
            })
        {
            return Err(StartError::PortInUse(holder));
        }

        // Residency admission (F10): only when we actually know the VRAM budget.
        // Reap first so a crashed endpoint is not counted as holding VRAM, then
        // arbitrate — evicting least-recently-used residents or refusing outright.
        if req.vram_budget > 0 && req.vram_need > 0 {
            let protected: HashSet<&str> = leases
                .values()
                .map(|lease| lease.endpoint_id.as_str())
                .collect();
            let mut residents: Vec<ResidentVram> = map
                .values_mut()
                .filter_map(|e| {
                    e.refresh();
                    (e.id != id && e.process_running() && e.vram_bytes > 0).then(|| ResidentVram {
                        id: e.id.clone(),
                        vram_bytes: e.vram_bytes,
                        last_used: e.last_used,
                    })
                })
                .collect();
            exclude_leased_residents(&mut residents, &protected);
            match admit(req.vram_budget, req.vram_need, &mut residents) {
                Admission::Admit => {}
                Admission::Evict(ids) => {
                    if !req.allow_evict {
                        return Err(StartError::EvictionRequired(ids));
                    }
                    eviction_ids = ids;
                }
                Admission::Refuse => {
                    let protected_vram: u64 = map
                        .values_mut()
                        .filter(|e| protected.contains(e.id.as_str()))
                        .map(|e| {
                            e.refresh();
                            if e.process_running() {
                                e.vram_bytes
                            } else {
                                0
                            }
                        })
                        .sum();
                    if protected_vram > 0 {
                        return Err(StartError::LeasedCapacity(format!(
                            "model needs ~{:.1} GiB of VRAM, but ~{:.1} GiB is reserved by active session leases; release a lease or stop that endpoint explicitly",
                            gib(req.vram_need),
                            gib(protected_vram),
                        )));
                    }
                    return Err(StartError::WontFit(format!(
                        "model needs ~{:.1} GiB of VRAM but the GPU has ~{:.1} GiB; \
                         quantize further, pick a smaller model, or add a GPU.",
                        gib(req.vram_need),
                        gib(req.vram_budget),
                    )));
                }
            }
        }

        // Commit stops first; never persist a replacement start before capacity is confirmed released.
        if !eviction_ids.is_empty() {
            self.commit_intent(&eviction_ids, None)
                .map_err(StartError::Persistence)?;
        }
        for victim in &eviction_ids {
            if let Some(endpoint) = map.get_mut(victim) {
                endpoint.error = Some("eviction requested; awaiting confirmed exit".into());
                endpoint.ready = false;
            }
        }
        for victim in eviction_ids {
            if let Some(endpoint) = map.get_mut(&victim) {
                endpoint.stop_owned().map_err(StartError::Termination)?;
            }
            map.remove(&victim);
        }
        self.commit_intent(&[], Some((&id, &req.intent)))
            .map_err(StartError::Persistence)?;
        let (child, error) = match spawn(&req.command) {
            Ok(child) => (Some(child), None),
            Err(e) => (None, Some(e.to_string())),
        };

        let mut endpoint = Endpoint {
            restart_enabled: true,
            id: id.clone(),
            model: req.model,
            host: req.host,
            port: req.port,
            backend: req.backend,
            fits_vram: req.fits_vram,
            notes: req.notes,
            context_tokens: req.context_tokens,
            command: req.command,
            child,
            error,
            exit_code: None,
            started_at: SystemTime::now(),
            restarts: 0,
            last_exit_at: None,
            vram_bytes: req.vram_need,
            last_used: SystemTime::now(),
            ready: false,
            last_health_probe: None,
            readiness_error: None,
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
            (e.model == model && e.is_ready()).then(|| (e.host.clone(), e.port, e.id.clone()))
        })
    }

    /// Distinct model names currently served (running), for `GET /v1/models`.
    pub fn served_models(&self) -> Vec<String> {
        let mut map = self.endpoints.lock().unwrap();
        let mut names: Vec<String> = map
            .values_mut()
            .filter_map(|e| {
                e.refresh();
                e.is_ready().then(|| e.model.clone())
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Safe, compact endpoint facts for the harness engine descriptor. Detailed
    /// command lines and ports stay on the operator-only server API.
    pub fn context_tokens_for_model(&self, model: &str) -> Option<u32> {
        self.engine_profiles(false)
            .into_iter()
            .find(|profile| profile["model"].as_str() == Some(model))
            .and_then(|profile| {
                profile["context_tokens"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
            })
    }

    pub fn engine_profiles(&self, include_vram: bool) -> Vec<Value> {
        let mut map = self.endpoints.lock().unwrap();
        let leases = self.leases.lock().unwrap();
        let mut profiles: Vec<Value> = map
            .values_mut()
            .filter_map(|endpoint| {
                endpoint.refresh();
                endpoint.is_ready().then(|| {
                    let lease_count = leases
                        .values()
                        .filter(|lease| lease.endpoint_id == endpoint.id)
                        .count();
                    let mut profile = json!({
                        "id": endpoint.id,
                        "model": endpoint.model,
                        "state": endpoint.state(),
                        "backend": endpoint.backend,
                        "context_tokens": endpoint.context_tokens,
                        "lease_count": lease_count,
                    });
                    if include_vram {
                        profile["vram_bytes"] = json!(endpoint.vram_bytes);
                    }
                    profile
                })
            })
            .collect();
        profiles.sort_by(|a, b| a["model"].as_str().cmp(&b["model"].as_str()));
        profiles
    }

    /// Claim a resident model for a session. Claims are opt-in: ordinary
    /// inference traffic remains eligible for LRU eviction.
    pub fn lease(&self, session_id: &str, model: &str) -> Result<Value, LeaseError> {
        let mut map = self.endpoints.lock().unwrap();
        let mut leases = self.leases.lock().unwrap();
        let endpoint_id = map.values_mut().find_map(|endpoint| {
            endpoint.refresh();
            (endpoint.model == model && endpoint.is_ready()).then(|| endpoint.id.clone())
        });
        let Some(endpoint_id) = endpoint_id else {
            return Err(LeaseError::Unavailable(model.to_string()));
        };
        let lease = Lease {
            session_id: session_id.to_string(),
            model: model.to_string(),
            endpoint_id,
            recovered: false,
            expires_at: epoch_seconds().saturating_add(LEASE_TTL_SECS),
            renewed_at: Some(std::time::Instant::now()),
        };
        let mut next = leases.clone();
        next.insert(session_id.to_string(), lease.clone());
        self.persist_leases(&next)
            .map_err(LeaseError::Persistence)?;
        let view = lease_view(&lease, &mut map);
        leases.insert(session_id.to_string(), lease);
        Ok(view)
    }

    /// Drop one session's claim. The endpoint stays running; it simply becomes
    /// eligible for normal LRU admission again.
    pub fn release(&self, session_id: &str) -> Result<bool, String> {
        let mut leases = self.leases.lock().unwrap();
        if !leases.contains_key(session_id) {
            return Ok(false);
        }
        let mut next = leases.clone();
        next.remove(session_id);
        self.persist_leases(&next)?;
        *leases = next;
        Ok(true)
    }

    /// Heartbeats extend active ownership, but cannot silently reclaim restored leases.
    pub fn renew_lease(&self, session_id: &str) -> Result<(), String> {
        let mut leases = self.leases.lock().unwrap();
        let Some(lease) = leases.get(session_id) else {
            return Ok(());
        };
        if lease.recovered {
            return Ok(());
        }
        if lease.expired(epoch_seconds()) {
            let mut next = leases.clone();
            next.remove(session_id);
            self.persist_leases(&next)?;
            *leases = next;
            return Ok(());
        }
        let mut next = leases.clone();
        let lease = next.get_mut(session_id).unwrap();
        lease.expires_at = epoch_seconds().saturating_add(LEASE_TTL_SECS);
        lease.renewed_at = Some(std::time::Instant::now());
        self.persist_leases(&next)?;
        *leases = next;
        Ok(())
    }

    fn expire_leases(&self, now: u64) -> Result<(), String> {
        let mut leases = self.leases.lock().unwrap();
        let mut next = leases.clone();
        next.retain(|_, lease| !lease.expired(now));
        if next.len() != leases.len() {
            self.persist_leases(&next)?;
            *leases = next;
        }
        Ok(())
    }

    /// Report an active lease or the explicit unavailable state when its
    /// endpoint died or was stopped by an operator.
    pub fn lease_status(&self, session_id: &str) -> Option<Value> {
        let mut map = self.endpoints.lock().unwrap();
        let leases = self.leases.lock().unwrap();
        leases
            .get(session_id)
            .map(|lease| lease_view(lease, &mut map))
    }

    /// The current view of every tracked endpoint, most-recently-started first.
    pub fn list(&self) -> Vec<Value> {
        let mut map = self.endpoints.lock().unwrap();
        let leases = self.leases.lock().unwrap();
        let mut views: Vec<(SystemTime, Value)> = map
            .values_mut()
            .map(|e| {
                let mut view = e.view();
                view["lease_count"] = json!(leases
                    .values()
                    .filter(|lease| lease.endpoint_id == e.id)
                    .count());
                (e.started_at, view)
            })
            .collect();
        views.sort_by_key(|v| std::cmp::Reverse(v.0));
        let mut result: Vec<Value> = views.into_iter().map(|(_, v)| v).collect();
        for (id, intent) in self.recovery_intents() {
            if !map.contains_key(&id) {
                result.push(json!({"id": id, "model": intent["model"], "state": "recovery_required",
                    "error": "Persisted endpoint could not be recovered; inspect configuration, model and hardware, then start explicitly.",
                    "lease_count": 0}));
            }
        }
        result
    }

    /// One endpoint's view by id, or `None` if unknown.
    pub fn get(&self, id: &str) -> Option<Value> {
        let mut map = self.endpoints.lock().unwrap();
        map.get_mut(id).map(Endpoint::view).or_else(|| {
            self.recovery_intents()
                .into_iter()
                .find(|(key, _)| key == id)
                .map(|(_, intent)| {
                    json!({
                        "id": id, "model": intent["model"], "state": "recovery_required",
                        "error": "Persisted endpoint requires operator recovery", "lease_count": 0
                    })
                })
        })
    }

    /// Stop and forget an endpoint only after observing exit. Unknown ids return
    /// false; failures retain the owned handle and disable automatic restart.
    pub fn stop(&self, id: &str) -> Result<bool, String> {
        let mut map = self.endpoints.lock().unwrap();
        let persisted = self.recovery_intents().iter().any(|(key, _)| key == id);
        if !map.contains_key(id) && !persisted {
            return Ok(false);
        }
        self.commit_intent(&[id.to_owned()], None)?;
        if let Some(endpoint) = map.get_mut(id) {
            endpoint.stop_owned()?;
        }
        map.remove(id);
        Ok(true)
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
            let running = if e.process_running() { 1 } else { 0 };
            let ready = if e.is_ready() { 1 } else { 0 };
            let labels = format!(
                "id=\"{}\",model=\"{}\",port=\"{}\",backend=\"{}\",state=\"{}\"",
                esc(&e.id),
                esc(&e.model),
                e.port,
                esc(&e.backend),
                e.state()
            );
            up.push_str(&format!("cameo_endpoint_up{{{labels}}} {running}\n"));
            up.push_str(&format!("cameo_endpoint_ready{{{labels}}} {ready}\n"));
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
        out.push_str("# HELP cameo_endpoint_ready 1 if the endpoint passed its health probe.\n");
        out.push_str("# TYPE cameo_endpoint_ready gauge\n");
        out.push_str(&up);
        out.push_str("# HELP cameo_endpoint_restarts_total Auto-restarts since creation.\n");
        out.push_str("# TYPE cameo_endpoint_restarts_total counter\n");
        out.push_str(&restarts);
        out.push_str(
            "# HELP cameo_endpoint_uptime_seconds Seconds since the current process started.\n",
        );
        out.push_str("# TYPE cameo_endpoint_uptime_seconds gauge\n");
        out.push_str(&uptime);
        out.push_str("# HELP cameo_endpoint_vram_bytes Estimated VRAM the endpoint holds.\n");
        out.push_str("# TYPE cameo_endpoint_vram_bytes gauge\n");
        out.push_str(&vram);
        out
    }
}

/// Render a lease without hiding a stopped or failed endpoint. A client can
/// distinguish a clean release (404 after `DELETE`) from a claim whose backing
/// model is no longer usable and decide whether to re-ensure it.
fn lease_view(lease: &Lease, endpoints: &mut HashMap<String, Endpoint>) -> Value {
    let state = if lease.expired(epoch_seconds()) {
        "expired"
    } else if lease.recovered {
        "recovery_required"
    } else {
        match endpoints.get_mut(&lease.endpoint_id) {
            Some(endpoint) => {
                endpoint.refresh();
                if endpoint.is_ready() {
                    "active"
                } else {
                    "unavailable"
                }
            }
            None => "unavailable",
        }
    };
    json!({
        "session_id": lease.session_id,
        "model": lease.model,
        "endpoint_id": lease.endpoint_id,
        "state": state,
        "expires_at": lease.expires_at,
    })
}

/// Keep explicit session claims out of normal LRU eviction. The lease remains
/// observable even if its endpoint stops; only a running resident reaches this
/// selection helper, so stopped claims cannot consume admission capacity.
fn exclude_leased_residents(residents: &mut Vec<ResidentVram>, protected: &HashSet<&str>) {
    residents.retain(|resident| !protected.contains(resident.id.as_str()));
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
            secret_env: Vec::new(),
        }
    }

    fn req(model: &str, port: u16) -> StartRequest {
        StartRequest {
            intent: json!({"model": model, "port": port}),
            model: model.into(),
            host: "127.0.0.1".into(),
            port,
            backend: "Vulkan".into(),
            fits_vram: true,
            notes: vec![],
            context_tokens: 4096,
            command: spec(),
            vram_need: 0,
            vram_budget: 0,
            allow_evict: true,
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
    fn leased_residents_are_excluded_from_lru_eviction() {
        let mut r = vec![resident("leased", 8, 100), resident("ordinary", 4, 50)];
        let protected: HashSet<&str> = ["leased"].into_iter().collect();

        exclude_leased_residents(&mut r, &protected);

        assert_eq!(
            admit(8, 8, &mut r),
            Admission::Evict(vec!["ordinary".into()])
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
        assert_eq!(view["context_tokens"], 4096);
        assert!(view["error"].is_string());

        let listed = sup.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["lease_count"], 0);
        assert!(sup.get("tinyllama-8080").is_some());
    }

    #[test]
    fn desired_state_survives_restart_without_secrets_or_phantom_readiness() {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let dir =
            std::env::temp_dir().join(format!("cameo-supervisor-{:x}", u64::from_le_bytes(random)));
        let sup = Supervisor::open(&dir).unwrap();
        let mut request = req("tinyllama", 8080);
        request
            .command
            .secret_env
            .push(("LLAMA_API_KEY".into(), "canary-never-on-disk".into()));
        sup.start(request).unwrap();
        drop(sup);
        let recovered = Supervisor::open(&dir).unwrap();
        assert_eq!(recovered.recovery_intents().len(), 1);
        assert_eq!(recovered.list()[0]["state"], "recovery_required");
        assert!(recovered.endpoint_for_model("tinyllama").is_none());
        for item in std::fs::read_dir(&dir).unwrap() {
            let path = item.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let data = std::fs::read_to_string(path).unwrap();
                assert!(!data.contains("canary-never-on-disk"));
                assert!(!data.contains("secret_env"));
            }
        }
        assert!(recovered.stop("tinyllama-8080").unwrap());
        drop(recovered);
        let stopped = Supervisor::open(&dir).unwrap();
        assert!(stopped.has_history());
        assert!(stopped.recovery_intents().is_empty());
        drop(stopped);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_persistence_does_not_start_an_endpoint() {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let dir =
            std::env::temp_dir().join(format!("cameo-state-fail-{:x}", u64::from_le_bytes(random)));
        let sup = Supervisor::open(&dir).unwrap();
        // A colliding generation represents a failed/ambiguous previous commit.
        std::fs::write(dir.join("state-00000000000000000001.json"), b"{}").unwrap();
        assert!(matches!(
            sup.start(req("tinyllama", 8080)),
            Err(StartError::Persistence(_))
        ));
        assert!(sup.list().is_empty());
        drop(sup);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn leasing_a_missing_model_never_starts_it() {
        let sup = Supervisor::new();
        assert!(matches!(
            sup.lease("session-a", "not-running"),
            Err(LeaseError::Unavailable(model)) if model == "not-running"
        ));
        assert!(sup.lease_status("session-a").is_none());
        assert!(!sup.release("session-a").unwrap());
        assert!(
            sup.list().is_empty(),
            "leasing is not a hidden load operation"
        );
    }

    #[test]
    fn stop_forgets_the_endpoint_and_reports_unknown_ids() {
        let sup = Supervisor::new();
        sup.start(req("tinyllama", 8080)).unwrap();
        assert!(sup.stop("tinyllama-8080").unwrap());
        assert!(sup.get("tinyllama-8080").is_none());
        assert!(!sup.stop("tinyllama-8080").unwrap());
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
        assert!(
            m.contains(r#"cameo_endpoint_up{id="tinyllama-8080",model="tinyllama",port="8080""#)
        );
        assert!(m.contains(r#"cameo_endpoint_restarts_total{id="tinyllama-8080"} 0"#));
    }

    #[test]
    fn label_values_are_escaped() {
        assert_eq!(esc(r#"a"b\c"#), r#"a\"b\\c"#);
        assert_eq!(esc("line\nbreak"), "line\\nbreak");
        assert_eq!(esc("plain"), "plain");
    }

    #[test]
    fn a_stable_run_earns_back_the_restart_budget() {
        // Quick crash: the count is kept. Long healthy run: the slate is wiped.
        assert_eq!(restarts_after_exit(3, Duration::from_secs(5)), 3);
        assert_eq!(restarts_after_exit(3, STABLE_UPTIME_RESET), 0);
        assert_eq!(
            restarts_after_exit(MAX_RESTARTS, Duration::from_secs(3600)),
            0
        );
    }

    #[test]
    fn a_failed_holder_does_not_block_a_different_model_on_its_port() {
        // On this dev host every spawn fails, so the first endpoint is `failed`,
        // not `running` — a second model on the same port must be admitted (the
        // port check only guards against a *live* holder).
        let sup = Supervisor::new();
        sup.start(req("tinyllama", 8080)).unwrap();
        assert!(sup.start(req("qwen", 8080)).is_ok());
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

    #[test]
    fn durable_lease_recovery_and_transactional_release() {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let dir = std::env::temp_dir().join(format!("cameo-leases-{}", u64::from_le_bytes(random)));
        let sup = Supervisor::open(&dir).unwrap();
        let leases = HashMap::from([(
            "session".into(),
            Lease {
                session_id: "session".into(),
                model: "fixture".into(),
                endpoint_id: "fixture-8080".into(),
                recovered: false,
                expires_at: epoch_seconds().saturating_add(LEASE_TTL_SECS),
                renewed_at: Some(std::time::Instant::now()),
            },
        )]);
        sup.persist_leases(&leases).unwrap();
        drop(sup);
        let recovered = Supervisor::open(&dir).unwrap();
        assert_eq!(
            recovered.lease_status("session").unwrap()["state"],
            "recovery_required"
        );
        let expiry = recovered.lease_status("session").unwrap()["expires_at"]
            .as_u64()
            .unwrap();
        recovered.renew_lease("session").unwrap();
        assert_eq!(
            recovered.lease_status("session").unwrap()["expires_at"],
            expiry,
            "heartbeat does not silently reclaim a recovered lease"
        );
        assert!(recovered.lease("session", "fixture").is_err());
        let collision = dir.join("state-00000000000000000002.json");
        std::fs::write(&collision, b"collision").unwrap();
        assert!(recovered.release("session").is_err());
        assert!(recovered.expire_leases(expiry).is_err());
        assert!(
            recovered.lease_status("session").is_some(),
            "failed durable release retains ownership"
        );
        std::fs::remove_file(collision).unwrap();
        assert!(recovered.release("session").unwrap());
        drop(recovered);
        assert!(Supervisor::open(&dir)
            .unwrap()
            .lease_status("session")
            .is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn orphan_expiry_survives_restart_and_clock_rollback_is_bounded() {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let dir = std::env::temp_dir().join(format!("cameo-expiry-{}", u64::from_le_bytes(random)));
        let sup = Supervisor::open(&dir).unwrap();
        let leases = HashMap::from([(
            "orphan".into(),
            Lease {
                session_id: "orphan".into(),
                model: "fixture".into(),
                endpoint_id: "fixture-8080".into(),
                recovered: false,
                expires_at: u64::MAX,
                renewed_at: None,
            },
        )]);
        sup.persist_leases(&leases).unwrap();
        drop(sup);
        let recovered = Supervisor::open(&dir).unwrap();
        let expiry = recovered.lease_status("orphan").unwrap()["expires_at"]
            .as_u64()
            .unwrap();
        assert!(expiry <= epoch_seconds() + LEASE_TTL_SECS);
        drop(recovered);
        let reopened = Supervisor::open(&dir).unwrap();
        assert_eq!(
            reopened.lease_status("orphan").unwrap()["expires_at"],
            expiry,
            "another restart cannot extend the recovery window"
        );
        reopened.expire_leases(expiry).unwrap();
        assert!(reopened.lease_status("orphan").is_none());
        drop(reopened);
        assert!(Supervisor::open(&dir)
            .unwrap()
            .lease_status("orphan")
            .is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn heartbeat_cannot_resurrect_expired_ownership() {
        let sup = Supervisor::new();
        sup.leases.lock().unwrap().insert(
            "late".into(),
            Lease {
                session_id: "late".into(),
                model: "fixture".into(),
                endpoint_id: "fixture-8080".into(),
                recovered: false,
                expires_at: epoch_seconds().saturating_sub(1),
                renewed_at: None,
            },
        );
        assert_eq!(sup.lease_status("late").unwrap()["state"], "expired");
        sup.renew_lease("late").unwrap();
        assert!(sup.lease_status("late").is_none());
    }

    #[test]
    fn session_identity_recovers_without_mission_text_and_deletion_is_durable() {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let dir =
            std::env::temp_dir().join(format!("cameo-sessions-{}", u64::from_le_bytes(random)));
        let sup = Supervisor::open(&dir).unwrap();
        let session: crate::sessions::Session = serde_json::from_value(json!({"id":"owner", "name":"builder",
            "model":"fixture", "task":"private-mission-canary", "workspace":"private-workspace-canary"})).unwrap();
        sup.persist_session(&session).unwrap();
        drop(sup);
        let recovered = Supervisor::open(&dir).unwrap();
        let records = recovered.recovery_sessions();
        let serialized = serde_json::to_string(&records).unwrap();
        assert!(!serialized.contains("canary"));
        let board = crate::sessions::Board::recover(records).unwrap();
        assert_eq!(board.get("owner").unwrap().name, "builder");
        assert!(!board.contains("owner"));
        recovered.remove_session("owner").unwrap();
        drop(recovered);
        assert!(Supervisor::open(&dir)
            .unwrap()
            .recovery_sessions()
            .is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn termination_refuses_unobserved_exit_and_handles_exit_race() {
        struct Fixture {
            polls: usize,
            exited_after: usize,
            kill_fails: bool,
        }
        impl ManagedProcess for Fixture {
            fn exited(&mut self) -> std::io::Result<bool> {
                self.polls += 1;
                Ok(self.polls >= self.exited_after)
            }
            fn terminate(&mut self) -> std::io::Result<()> {
                if self.kill_fails {
                    Err(std::io::Error::other("injected termination failure"))
                } else {
                    Ok(())
                }
            }
        }
        assert!(confirm_termination(
            &mut Fixture {
                polls: 0,
                exited_after: usize::MAX,
                kill_fails: true
            },
            Duration::ZERO
        )
        .is_err());
        assert!(confirm_termination(
            &mut Fixture {
                polls: 0,
                exited_after: usize::MAX,
                kill_fails: false
            },
            Duration::ZERO
        )
        .is_err());
        assert!(
            confirm_termination(
                &mut Fixture {
                    polls: 0,
                    exited_after: 2,
                    kill_fails: true
                },
                Duration::ZERO
            )
            .is_ok(),
            "exit racing with kill is observed as success"
        );
    }

    #[test]
    fn owned_process_exit_is_reaped() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "supervisor::tests::owned_process_fixture"])
            .env("CAMEO_OWNED_PROCESS_FIXTURE", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().unwrap();
        let result = confirm_termination(&mut child, Duration::from_secs(2));
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result.unwrap();
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn owned_process_fixture() {
        if std::env::var("CAMEO_OWNED_PROCESS_FIXTURE").as_deref() == Ok("1") {
            std::thread::sleep(Duration::from_secs(30));
        }
    }

    #[test]
    fn planned_shutdown_preserves_intent_and_prevents_new_starts() {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let dir =
            std::env::temp_dir().join(format!("cameo-shutdown-{}", u64::from_le_bytes(random)));
        let sup = Supervisor::open(&dir).unwrap();
        sup.start(req("fixture", 19090)).unwrap();
        let intents = sup.recovery_intents();
        sup.shutdown_owned().unwrap();
        assert!(matches!(
            sup.start(req("other", 19091)),
            Err(StartError::ShuttingDown)
        ));
        assert_eq!(sup.recovery_intents(), intents);
        drop(sup);
        assert_eq!(Supervisor::open(&dir).unwrap().recovery_intents(), intents);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
