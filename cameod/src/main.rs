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
mod dashboard;
mod dispatch;
mod http;
mod hub;
mod proxy;
mod sessions;
mod supervisor;

#[derive(Parser)]
#[command(
    name = "cameod",
    version,
    about = "cameod — the Cameo control plane: a browser-administered console for AMD-GPU inference hosting."
)]
struct Args {
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

    /// Run as a fleet **hub**: serve the central dashboard at `/` and accept nodes
    /// at `/hub/register`. Requires --farm-token. Reads `CAMEO_HUB`.
    #[arg(long, env = "CAMEO_HUB")]
    hub: bool,

    /// The shared farm secret. In hub mode, the token a node must present to
    /// enroll; on a node with --hub-url, the token this node presents to the hub.
    #[arg(long, value_name = "TOKEN", env = "CAMEO_FARM_TOKEN")]
    farm_token: Option<String>,

    /// Phone home to this hub on boot (node mode), e.g. `http://hub.lan:9090`.
    /// Requires --farm-token. Reads `CAMEO_HUB_URL`.
    #[arg(long, value_name = "URL", env = "CAMEO_HUB_URL")]
    hub_url: Option<String>,

    /// The `host:port` the hub should call back to reach this node's `/api`.
    /// Defaults to this daemon's own host:port, which is wrong when bound to
    /// `0.0.0.0` — set it to the node's LAN address in that case. Reads
    /// `CAMEO_ADVERTISE_ADDR`.
    #[arg(long, value_name = "HOSTPORT", env = "CAMEO_ADVERTISE_ADDR")]
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

    // Hub mode requires a farm token: an open registration endpoint would let any
    // LAN peer enroll a node and drive the fleet. Fail closed at startup.
    if args.hub && args.farm_token.is_none() {
        return Err(anyhow!(
            "--hub requires --farm-token (or CAMEO_FARM_TOKEN): nodes must authenticate to enroll."
        ));
    }

    let state = Arc::new(AppState {
        sup: Supervisor::new(),
        captures,
        settings,
        keyring,
        posture,
        detect_cache: std::sync::Mutex::new(None),
        board: Board::new(),
        farm: hub::Farm::new(),
        hub_enabled: args.hub,
        farm_token: args.farm_token.clone(),
        // /v1 is only open without a credential on a loopback bind, or when the
        // operator explicitly opts in. A routable bind with an operator-only key
        // ring keeps /v1 gated to that key rather than serving the GPU to the LAN.
        open_inference: is_loopback(&args.host)
            || std::env::var_os("CAMEO_OPEN_INFERENCE").is_some(),
    });

    // Node mode: if told where the hub is, phone home in the background. The agent
    // sends this box's own /api/node description and heartbeats on an interval.
    if let Some(hub_url) = args.hub_url.clone() {
        let token = args.farm_token.clone().ok_or_else(|| {
            anyhow!(
                "--hub-url requires --farm-token (or CAMEO_FARM_TOKEN) to authenticate to the hub."
            )
        })?;
        let callback = args
            .advertise
            .clone()
            .unwrap_or_else(|| format!("{}:{}", args.host, args.port));
        let cfg = agent::HubConfig {
            hub_url: hub_url.clone(),
            farm_token: token,
            node_name: app::node_name(),
            callback_address: callback,
            node_key: args.console_key.clone(),
        };
        let agent_state = Arc::clone(&state);
        agent::spawn(cfg, move || app::node_report(&agent_state));
        eprintln!("cameod: phoning home to hub at {hub_url}");
    }

    let listener = TcpListener::bind((args.host.as_str(), args.port))
        .map_err(|e| anyhow!("binding {}:{}: {e}", args.host, args.port))?;

    eprintln!(
        "cameod: {} on http://{}:{} [{}]{}",
        if args.hub { "fleet hub" } else { "console" },
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

    http::serve(listener, move |req| app::route(&state, req));
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

/// Read an optional capture file into its contents, preserving path context.
fn read_opt(path: &Option<PathBuf>) -> Result<Option<String>> {
    match path {
        Some(p) => std::fs::read_to_string(p)
            .map(Some)
            .map_err(|e| anyhow!("reading {}: {e}", p.display())),
        None => Ok(None),
    }
}
