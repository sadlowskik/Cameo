//! `cameod` — the Cameo control-plane daemon.
//!
//! An appliance you administer from a browser: it serves a self-contained
//! dashboard and a small JSON API over the same detection and placement brain
//! the `cameo` CLI uses, and it supervises the model endpoints it starts so they
//! outlive a single command. One binary, no external web stack — the HTTP server
//! is [`http`], the routing is [`app`], the process bookkeeping is [`supervisor`].
//!
//! Live GPU detection needs Linux; on a dev host, pass captured tool output with
//! `--lspci-file` (and friends) exactly as the CLI does, and the whole console —
//! GPU report, planning, endpoint list — works, with the final spawn the only
//! step that reports "Linux only".

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use clap::Parser;

use cameo_config::Settings;
use cameo_gpu_detect::Captures;

use crate::app::AppState;
use crate::sessions::Board;
use crate::supervisor::Supervisor;

mod agent;
mod app;
mod auth;
mod curl;
mod dashboard;
mod dispatch;
mod drain;
mod endpoint_store;
mod http;
mod hub;
mod openai;
mod pairing;
mod proxy;
mod rate_limit;
mod resolve;
mod sessions;
mod supervisor;

#[derive(Parser)]
#[command(
    name = "cameod",
    version,
    about = "cameod — the Cameo control plane: a browser-administered console for AMD-GPU inference hosting."
)]
struct Args {
    #[command(subcommand)]
    maintenance: Option<Maintenance>,
    /// Address to bind the console to. Anything but loopback requires --console-key.
    /// Reads `CAMEO_CONSOLE_HOST` so the shipped systemd unit is configurable via
    /// `/etc/cameo/cameod.env` without editing the unit.
    #[arg(long, default_value = "127.0.0.1", env = "CAMEO_CONSOLE_HOST")]
    host: String,

    /// Port to listen on. Reads `CAMEO_CONSOLE_PORT`.
    #[arg(long, default_value_t = 9090, env = "CAMEO_CONSOLE_PORT")]
    port: u16,

    /// The primary **operator** key (as `Authorization: Bearer …`): full control of
    /// `/api` and `/hub`. Mandatory when binding to anything other than loopback.
    #[arg(long, value_name = "KEY", env = "CAMEO_CONSOLE_KEY")]
    console_key: Option<String>,

    /// Deployment posture: `self-host` (default) lets a co-located harness get
    /// keyless GPU control via the privileged local socket; `multi-tenant`
    /// (`enterprise`) disables it and requires an operator key. Reads `CAMEO_POSTURE`.
    #[arg(long, default_value = "self-host", env = "CAMEO_POSTURE")]
    posture: String,

    /// A JSON file of additional role-tagged API keys:
    /// `[{ "key":"…", "role":"operator|consumer", "label":"friend-bob" }]`.
    /// Consumer keys reach inference (`/v1`) only; operator keys reach the control
    /// surface. Reads `CAMEO_KEYS_FILE`.
    #[arg(long, value_name = "FILE", env = "CAMEO_KEYS_FILE")]
    keys_file: Option<PathBuf>,

    /// Load a daemon config file (TOML): backend, hsa_override, serve_api_key, …
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Durable daemon state directory. Paired mesh identities are stored here.
    /// Defaults to /var/lib/cameo on Unix and the user's local app-data on Windows.
    #[arg(long, value_name = "DIR", env = "CAMEO_STATE_DIR")]
    state_dir: Option<PathBuf>,

    /// Run as a Cameo Mesh hub: serve the central dashboard and accept paired
    /// Cameo Link nodes. A farm token enables only the legacy join path.
    #[arg(long, env = "CAMEO_HUB")]
    hub: bool,

    /// Legacy shared fleet secret. Prefer one-time pairing and per-node identity.
    #[arg(long, value_name = "TOKEN", env = "CAMEO_FARM_TOKEN")]
    farm_token: Option<String>,

    /// One-time code created by `POST /hub/pairings` for device-bound enrollment.
    /// Never written to disk. Reads `CAMEO_PAIRING_CODE`.
    #[arg(long, value_name = "64-HEX", env = "CAMEO_PAIRING_CODE")]
    pairing_code: Option<String>,

    /// Owner-only JSON file where a paired node stores its issued credential.
    /// Required with --pairing-code and reused on restart.
    #[arg(long, value_name = "FILE", env = "CAMEO_MESH_CREDENTIAL_FILE")]
    mesh_credential_file: Option<PathBuf>,

    /// Run Cameo Link and phone home to this Mesh hub on boot. Requires a pairing
    /// code, an existing mesh credential file, or an explicit legacy farm token.
    #[arg(long, value_name = "URL", env = "CAMEO_HUB_URL")]
    hub_url: Option<String>,

    /// The HTTPS base URL the hub should call to reach this node's `/api`.
    /// Required with --hub-url; normally points at a TLS reverse proxy in front
    /// of the node's loopback-only cameod. Reads `CAMEO_ADVERTISE_ADDR`.
    #[arg(long, value_name = "HTTPS_URL", env = "CAMEO_ADVERTISE_ADDR")]
    advertise: Option<String>,

    /// Read `lspci -D -nn` from a file instead of the live system (dev/testing).
    #[arg(long, value_name = "FILE")]
    lspci_file: Option<PathBuf>,

    /// Read `rocminfo` from a file instead of the live system (dev/testing).
    #[arg(long, value_name = "FILE")]
    rocminfo_file: Option<PathBuf>,

    /// Read `rocm-smi --showtopo` from a file (multi-GPU dev/testing).
    #[arg(long, value_name = "FILE")]
    topo_file: Option<PathBuf>,

    /// Read `/proc/meminfo` from a file (dev/testing of host-RAM sizing).
    #[arg(long, value_name = "FILE")]
    meminfo_file: Option<PathBuf>,

    /// Read captured `/sys/class/drm` memory facts (TOML) for VRAM/GTT sizing.
    #[arg(long, value_name = "FILE")]
    gpu_mem_file: Option<PathBuf>,
}

#[derive(clap::Subcommand)]
enum Maintenance {
    /// Offline endpoint-state verification, backup and restore. Stop the daemon first.
    State {
        #[command(subcommand)]
        action: StateAction,
    },
}

#[derive(clap::Subcommand)]
enum StateAction {
    /// Verify the committed endpoint snapshot under an exclusive ownership lock.
    Check { directory: PathBuf },
    /// Export verified endpoint intent to a new file (not a model/identity backup).
    Backup {
        directory: PathBuf,
        destination: PathBuf,
    },
    /// Verify a backup and restore endpoint intent into a new directory.
    Restore {
        source: PathBuf,
        destination: PathBuf,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("CAMEO_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    if let Err(e) = run(Args::parse()) {
        eprintln!("cameod: {e}");
        exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    if let Some(Maintenance::State { action }) = args.maintenance {
        use endpoint_store::EndpointStore;
        let report = match action {
            StateAction::Check { directory } => EndpointStore::inspect(&directory),
            StateAction::Backup { directory, destination } => EndpointStore::backup(&directory, &destination)
                .map(|_| serde_json::json!({"status":"verified_backup", "scope":"endpoint intents, lease ownership and session identity only"})),
            StateAction::Restore { source, destination } => EndpointStore::restore(&source, &destination)
                .map(|_| serde_json::json!({"status":"verified_restore", "scope":"endpoint intents, lease ownership and session identity only; processes not started"})),
        }.map_err(|e| anyhow!(e))?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let captures = Captures {
        lspci: read_opt(&args.lspci_file)?,
        rocminfo: read_opt(&args.rocminfo_file)?,
        topo: read_opt(&args.topo_file)?,
        meminfo: read_opt(&args.meminfo_file)?,
        gpu_mem: read_opt(&args.gpu_mem_file)?,
    };

    let file_settings = match &args.config {
        Some(path) => Settings::load_file(path)
            .map_err(|e| anyhow!("loading config {}: {e}", path.display()))?,
        None => Settings::default(),
    };
    let settings = cameo_config::resolve(Settings::default(), file_settings, Settings::default());

    if let Some(secret) = args.console_key.as_deref() {
        auth::validate_secret("console key", secret).map_err(|e| anyhow!(e))?;
    }
    if let Some(secret) = args.farm_token.as_deref() {
        auth::validate_secret("farm token", secret).map_err(|e| anyhow!(e))?;
    }
    if let Some(secret) = settings.serve_api_key.as_deref() {
        auth::validate_secret("serve API key", secret).map_err(|e| anyhow!(e))?;
    }

    // Honour the config file's `model_dir` by exporting it as the env var the
    // models crate already treats as authoritative. An env var the operator set
    // themselves still wins — config must not silently override an explicit
    // environment. Done here, before any thread spawns, so `set_var` is safe.
    if let Some(dir) = &settings.model_dir {
        if std::env::var_os("CAMEO_MODELS_DIR").is_none() {
            std::env::set_var("CAMEO_MODELS_DIR", dir);
        }
    }

    let posture = auth::Posture::parse(&args.posture).ok_or_else(|| {
        anyhow!(
            "invalid --posture '{}': use 'self-host' or 'multi-tenant'.",
            args.posture
        )
    })?;

    // Assemble the keyring: the console key is the primary operator credential, the
    // serve key a consumer credential, plus any role-tagged keys from --keys-file.
    let mut keys: Vec<auth::ApiKey> = Vec::new();
    if let Some(k) = &args.console_key {
        keys.push(auth::ApiKey {
            key: k.clone(),
            role: auth::Role::Operator,
            label: "console".into(),
        });
    }
    if let Some(k) = &settings.serve_api_key {
        keys.push(auth::ApiKey {
            key: k.clone(),
            role: auth::Role::Consumer,
            label: "serve".into(),
        });
    }
    if let Some(path) = &args.keys_file {
        keys.extend(auth::load_keys_file(path).map_err(|e| anyhow!("{e}"))?);
    }
    for k in &keys {
        tracing::debug!(role = ?k.role, label = %k.label, "loaded api key");
    }
    let keyring = auth::KeyRing::new(keys);
    let operator_required = keyring.requires_operator();
    let node_key = keyring.operator_key().map(str::to_owned);

    // A routable control surface with no operator credential is an open door to the
    // machine's GPUs; refuse it the same way `cameo serve` refuses a public endpoint.
    if !is_loopback(&args.host) && !operator_required {
        return Err(anyhow!(
            "refusing to bind {} without an operator key (--console-key, or an operator \
             entry in --keys-file). Bind to 127.0.0.1 for local administration.",
            args.host
        ));
    }

    // Multi-tenant posture must have an operator credential: otherwise the control
    // surface is open and any tenant could manipulate another's VRAM. Fail closed.
    if posture == auth::Posture::MultiTenant && !operator_required {
        return Err(anyhow!(
            "multi-tenant posture requires at least one operator key (--console-key or an \
             operator entry in --keys-file), so tenants cannot manipulate the GPU."
        ));
    }

    // A hub may be pairing-only. Without a farm token, legacy registration fails
    // closed while operator-created one-time pairing codes remain available.

    let farm = if args.hub {
        let path = state_directory(args.state_dir.as_deref()).join("mesh-identities.json");
        hub::Farm::open(&path).map_err(|error| {
            anyhow!(
                "opening durable mesh identity state {}: {error}",
                path.display()
            )
        })?
    } else {
        hub::Farm::new()
    };

    let sup = Supervisor::open(&state_directory(args.state_dir.as_deref()).join("endpoints"))
        .map_err(|error| anyhow!("opening durable endpoint state: {error}"))?;
    let board = Board::recover(sup.recovery_sessions()).map_err(|error| anyhow!(error))?;
    let state = Arc::new(AppState {
        drain: drain::Drain::default(),
        sup,
        captures,
        settings,
        keyring,
        posture,
        detect_cache: std::sync::Mutex::new(None),
        board,
        farm,
        admissions: dispatch::AdmissionBook::new(),
        pairings: pairing::PairingStore::new(),
        rate_limits: rate_limit::RateLimiter::new(),
        hub_enabled: args.hub,
        farm_token: args.farm_token.clone(),
        // /v1 is only open without a credential on a loopback bind, or when the
        // operator explicitly opts in. A routable bind with an operator-only key
        // ring keeps /v1 gated to that key rather than serving the GPU to the LAN.
        open_inference: is_loopback(&args.host)
            || std::env::var_os("CAMEO_OPEN_INFERENCE").is_some(),
    });
    let shutdown_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal_flag = Arc::clone(&shutdown_requested);
    ctrlc::set_handler(move || {
        signal_flag.store(true, std::sync::atomic::Ordering::SeqCst);
    })
    .map_err(|error| anyhow!("installing shutdown handler: {error}"))?;
    let maintenance = Arc::clone(&state);
    std::thread::spawn(move || loop {
        maintenance.drain.poll();
        maintenance.sup.maintain();
        std::thread::sleep(std::time::Duration::from_millis(500));
    });

    // Node mode: if told where the hub is, phone home in the background. The agent
    // sends this box's own /api/node description and heartbeats on an interval.
    if let Some(hub_url) = args.hub_url.clone() {
        require_https_hub_url(&hub_url)?;
        let callback = args.advertise.clone().ok_or_else(|| {
            anyhow!(
                "--hub-url requires --advertise https://node.example:PORT so hub callbacks do not expose the node operator key"
            )
        })?;
        require_https_callback(&callback)?;
        if let Some(code) = args.pairing_code.as_deref() {
            if code.len() != 64 || !code.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(anyhow!(
                    "--pairing-code must be exactly 64 hexadecimal characters"
                ));
            }
            if args.mesh_credential_file.is_none() {
                return Err(anyhow!(
                    "--pairing-code requires --mesh-credential-file so the issued device identity survives restart"
                ));
            }
            if args
                .mesh_credential_file
                .as_ref()
                .is_some_and(|path| path.exists())
            {
                return Err(anyhow!(
                    "--pairing-code refuses to overwrite an existing mesh credential; omit the code to reconnect or remove the file intentionally before re-pairing"
                ));
            }
            if node_key.is_none() {
                return Err(anyhow!(
                    "paired nodes require an operator callback key (--console-key or operator keys-file entry)"
                ));
            }
        }
        let has_saved_credential = args
            .mesh_credential_file
            .as_ref()
            .is_some_and(|path| path.exists());
        if args.pairing_code.is_none() && !has_saved_credential && args.farm_token.is_none() {
            return Err(anyhow!(
                "--hub-url requires --pairing-code with --mesh-credential-file, an existing mesh credential file, or legacy --farm-token"
            ));
        }
        let cfg = agent::HubConfig {
            hub_url: hub_url.clone(),
            farm_token: args.farm_token.clone(),
            pairing_code: args.pairing_code.clone(),
            credential_file: args.mesh_credential_file.clone(),
            node_name: app::node_name(),
            callback_address: callback,
            node_key,
        };
        let agent_state = Arc::clone(&state);
        agent::spawn(cfg, move || app::node_report(&agent_state));
        eprintln!("cameod: phoning home to hub at {hub_url}");
    }

    let listener = TcpListener::bind((args.host.as_str(), args.port))
        .map_err(|e| anyhow!("binding {}:{}: {e}", args.host, args.port))?;
    app::recover_endpoints(&state);

    eprintln!(
        "cameod: {} on http://{}:{} [{}]{}",
        if args.hub {
            "Cameo Mesh hub"
        } else {
            "console"
        },
        args.host,
        args.port,
        match posture {
            auth::Posture::SelfHost => "self-host",
            auth::Posture::MultiTenant => "multi-tenant",
        },
        if operator_required {
            " (operator key required)"
        } else {
            ""
        }
    );
    if state.captures.is_live() {
        eprintln!("cameod: live GPU detection (Linux)");
    } else {
        eprintln!("cameod: detection replayed from captured fixtures");
    }

    if !args.hub {
        let boot = Arc::clone(&state);
        std::thread::spawn(move || app::maybe_autostart(&boot));
    }

    #[cfg(unix)]
    if posture == auth::Posture::SelfHost {
        let sock_path =
            std::env::var("CAMEO_SOCKET").unwrap_or_else(|_| "/run/cameo/cameo.sock".into());
        match bind_operator_socket(&sock_path) {
            Ok(unix) => {
                let sock_state = Arc::clone(&state);
                std::thread::spawn(move || {
                    http::serve_unix(unix, move |req| app::route(&sock_state, req));
                });
                eprintln!("cameod: operator socket {sock_path} (self-host, keyless for Knossos)");
            }
            Err(e) => {
                eprintln!("cameod: operator socket not bound ({e}); LAN HTTP is still keyed");
            }
        }
    }

    let serving = Arc::clone(&state);
    let mut shutdown_started = None;
    let mut shutdown_error = None;
    http::serve(
        listener,
        move |req| app::route(&serving, req),
        || {
            if !shutdown_requested.load(std::sync::atomic::Ordering::SeqCst) {
                return false;
            }
            let started = shutdown_started.get_or_insert_with(|| {
                state
                    .drain
                    .begin_shutdown(std::time::Duration::from_secs(30));
                state.sup.begin_shutdown();
                tracing::info!("shutdown requested; draining gateway for up to 30 seconds");
                std::time::Instant::now()
            });
            state.drain.poll();
            let idle = state.drain.status()["active_requests"].as_u64() == Some(0);
            if !idle && started.elapsed() < std::time::Duration::from_secs(35) {
                return false;
            }
            if !idle {
                tracing::warn!("shutdown cancellation grace expired with outstanding requests");
            }
            if let Err(error) = state.sup.shutdown_owned() {
                shutdown_error = Some(error);
            }
            true
        },
    )?;
    if let Some(error) = shutdown_error {
        return Err(anyhow!(error));
    }
    Ok(())
}

/// Host-only operator socket. 0600 so only the cameod user (and a co-located
/// harness running as the same user) can connect. Multi-tenant never calls this.
#[cfg(unix)]
fn bind_operator_socket(path: &str) -> Result<std::os::unix::net::UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir).map_err(|e| anyhow!("creating {}: {e}", dir.display()))?;
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).map_err(|e| anyhow!("binding {path}: {e}"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| anyhow!("chmod {path}: {e}"))?;
    Ok(listener)
}

/// Whether an address reaches this machine only.
fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

fn state_directory(configured: Option<&std::path::Path>) -> PathBuf {
    if let Some(path) = configured {
        return path.to_path_buf();
    }
    #[cfg(unix)]
    {
        PathBuf::from("/var/lib/cameo")
    }
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("PROGRAMDATA"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Cameo")
    }
    #[cfg(not(any(unix, windows)))]
    {
        PathBuf::from(".cameo")
    }
}

/// Mesh enrollment carries a node identity (or legacy farm token) and the
/// node's distinct callback operator key.
/// Refuse cleartext transport instead of letting either credential cross a LAN.
fn require_https_hub_url(url: &str) -> Result<()> {
    if url.starts_with("https://") {
        return Ok(());
    }
    Err(anyhow!(
        "refusing insecure hub URL '{url}': --hub-url carries privileged credentials and must use https://"
    ))
}

fn require_https_callback(url: &str) -> Result<()> {
    if url.starts_with("https://") {
        return Ok(());
    }
    Err(anyhow!(
        "refusing insecure callback URL '{url}': --advertise must use https://"
    ))
}

/// Read an optional capture file into its contents, preserving path context.
fn read_opt(path: &Option<PathBuf>) -> Result<Option<String>> {
    match path {
        Some(p) => std::fs::read_to_string(p)
            .map(Some)
            .map_err(|e| anyhow!("reading {}: {e}", p.display())),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_urls_must_use_https() {
        assert!(require_https_hub_url("https://hub.example:9090").is_ok());
        assert!(require_https_hub_url("http://hub.lan:9090").is_err());
        assert!(require_https_hub_url("hub.lan:9090").is_err());
        assert!(require_https_callback("https://node.example:9443").is_ok());
        assert!(require_https_callback("10.0.0.2:9090").is_err());
    }
}
