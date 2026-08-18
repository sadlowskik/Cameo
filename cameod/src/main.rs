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
use crate::supervisor::Supervisor;

mod app;
mod dashboard;
mod http;
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

    /// Require this key (as `Authorization: Bearer …`) on every `/api` request.
    /// Mandatory when binding to anything other than loopback.
    #[arg(long, value_name = "KEY", env = "CAMEO_CONSOLE_KEY")]
    console_key: Option<String>,

    /// Load a daemon config file (TOML): backend, hsa_override, serve_api_key, …
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

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
    // A routable console with no key is an open door to the machine's GPUs; refuse
    // it the same way `cameo serve` refuses an unauthenticated public endpoint.
    if !is_loopback(&args.host) && args.console_key.is_none() {
        return Err(anyhow!(
            "refusing to bind the console to {} without --console-key (or CAMEO_CONSOLE_KEY). \
             Bind to 127.0.0.1 for local administration.",
            args.host
        ));
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

    let state = Arc::new(AppState {
        sup: Supervisor::new(),
        captures,
        settings,
        console_key: args.console_key.clone(),
    });

    let listener = TcpListener::bind((args.host.as_str(), args.port))
        .map_err(|e| anyhow!("binding {}:{}: {e}", args.host, args.port))?;

    eprintln!(
        "cameod: console on http://{}:{}{}",
        args.host,
        args.port,
        if args.console_key.is_some() {
            " (console key required)"
        } else {
            ""
        }
    );
    if state.captures.is_live() {
        eprintln!("cameod: live GPU detection (Linux)");
    } else {
        eprintln!("cameod: detection replayed from captured fixtures");
    }

    http::serve(listener, move |req| app::route(&state, req));
    Ok(())
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
