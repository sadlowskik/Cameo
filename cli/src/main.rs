//! `cameo` — the command-line client for Cameo.
//!
//! Thin client: it detects the GPU topology, classifies each card's tier, runs
//! the placement engine to decide how to place work, and turns that plan into an
//! exact command line. Only the final spawn touches hardware — so `--dry-run`
//! exercises the entire brain on any OS.  All commands support `--json`.
//!
//! On non-Linux dev machines, live detection is unavailable; pass captured text
//! with `--lspci-file` / `--rocminfo-file` / `--topo-file` / `--meminfo-file` to
//! exercise everything.

use std::path::PathBuf;
use std::process::exit;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};

use cameo_config::{Backend, Settings};
use cameo_gpu_detect::{
    classify_topology, detect_topology_or_cpu, Captures, OverrideDb, TierAssessment, Topology,
};
use cameo_placement::command::{
    build_llama_run, build_llama_server, build_quantize, build_training,
};
use cameo_placement::{plan as make_plan, CommandSpec, ModelMeta, PlacementPlan, QuantLevel, Task};

/// Terminal styling. Zero-dependency ANSI, auto-off when piped, on a dumb
/// terminal, or when `NO_COLOR` / `CAMEO_NO_COLOR` is set. Cameo's signature
/// accent is a warm coral (the carved-shell namesake); tiers are colour-coded
/// green / amber / cyan.
mod style {
    use std::io::IsTerminal;
    use std::sync::OnceLock;

    fn color_enabled() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| {
            if std::env::var_os("NO_COLOR").is_some()
                || std::env::var_os("CAMEO_NO_COLOR").is_some()
            {
                return false;
            }
            // The first-boot tier report runs as a systemd unit, so its stdout is
            // a journal pipe and `is_terminal()` is false — the one screen most
            // users see was the one guaranteed to be monochrome. The unit sets
            // this to opt back in; suppression above still wins.
            if std::env::var_os("CAMEO_FORCE_COLOR").is_some() {
                return true;
            }
            std::io::stdout().is_terminal()
        })
    }

    fn paint(code: &str, s: &str) -> String {
        if color_enabled() {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn accent(s: &str) -> String {
        paint("38;5;209", s)
    }
    pub fn bold(s: &str) -> String {
        paint("1", s)
    }
    pub fn dim(s: &str) -> String {
        paint("2", s)
    }

    /// Colour a string by GPU tier: 1 green, 2 amber, 3 cyan.
    pub fn tier(n: u8, s: &str) -> String {
        let code = match n {
            1 => "1;32",
            2 => "1;33",
            _ => "1;36",
        };
        paint(code, s)
    }

    /// The Cameo wordmark + tagline.
    pub fn banner() -> String {
        let art = "\
 ██████╗ █████╗ ███╗   ███╗███████╗ ██████╗
██╔════╝██╔══██╗████╗ ████║██╔════╝██╔═══██╗
██║     ███████║██╔████╔██║█████╗  ██║   ██║
██║     ██╔══██║██║╚██╔╝██║██╔══╝  ██║   ██║
╚██████╗██║  ██║██║ ╚═╝ ██║███████╗╚██████╔╝
 ╚═════╝╚═╝  ╚═╝╚═╝     ╚═╝╚══════╝ ╚═════╝";
        format!(
            "{}\n  {}\n",
            accent(art),
            dim("any AMD card → a working LLM box")
        )
    }
}

#[derive(Parser)]
#[command(
    name = "cameo",
    version,
    about = "Cameo — run LLMs on any AMD GPU (Vulkan baseline, ROCm when available)"
)]
struct Cli {
    /// Emit machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    json: bool,

    /// Compute and print the plan + command without executing anything.
    #[arg(long, global = true)]
    dry_run: bool,

    /// Force an `HSA_OVERRIDE_GFX_VERSION` value (overrides tier detection).
    #[arg(long, global = true, value_name = "VER")]
    hsa_override: Option<String>,

    /// Plan a model even when it exceeds VRAM + host RAM (expect swapping).
    #[arg(long, global = true)]
    allow_oversize: bool,

    /// API key clients must present to a served model. Required to bind
    /// anything other than loopback.
    #[arg(long, global = true, value_name = "KEY", env = "CAMEO_API_KEY")]
    api_key: Option<String>,

    /// Read `lspci -nn` output from a file instead of the live system (dev/testing).
    #[arg(long, global = true, value_name = "FILE")]
    lspci_file: Option<PathBuf>,

    /// Read `rocminfo` output from a file instead of the live system (dev/testing).
    #[arg(long, global = true, value_name = "FILE")]
    rocminfo_file: Option<PathBuf>,

    /// Read `rocm-smi --showtopo` output from a file (multi-GPU dev/testing).
    #[arg(long, global = true, value_name = "FILE")]
    topo_file: Option<PathBuf>,

    /// Read `/proc/meminfo` from a file (dev/testing of host-RAM sizing).
    #[arg(long, global = true, value_name = "FILE")]
    meminfo_file: Option<PathBuf>,

    /// Read captured `/sys/class/drm` memory facts (TOML) instead of the live
    /// system. VRAM, GTT and memory type are the inputs the planner sizes
    /// against, and an `lspci` capture cannot carry them.
    #[arg(long, global = true, value_name = "FILE")]
    gpu_mem_file: Option<PathBuf>,

    /// Load a config file (TOML). CLI flags override its values.
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show detected GPU(s), topology, support tier, and selected backend.
    GpuStatus,
    /// Compute a placement plan for a model without running it.
    Plan(PlanArgs),
    /// Run inference on a model.
    Run(RunArgs),
    /// Serve a model over a persistent OpenAI-compatible HTTP endpoint.
    Serve(ServeArgs),
    /// Download a model into the local cache (alias, URL, or owner/repo:file.gguf).
    Pull(PullArgs),
    /// Quantize a model to a target level (e.g. Q4_K_M).
    Quantize(QuantizeArgs),
    /// Start a training run (Tier 1/2 only; refused on Tier 3).
    Train(TrainArgs),
    /// Manage the local model cache: list, disk usage, remove, clean.
    Model(ModelArgs),
    /// Print the install plan Cameo would apply for the detected hardware.
    Install,
}

/// Shared model-description flags. Until GGUF metadata parsing exists, these let
/// the planner size memory; sane defaults keep commands working for `--dry-run`.
#[derive(clap::Args, Clone)]
struct ModelOpts {
    /// Total parameters, in billions (for memory planning).
    #[arg(long, default_value_t = 7.0)]
    params: f64,
    /// Quantization level: F16, Q8_0, Q6_K, Q5_K_M, Q4_K_M, Q4_0.
    #[arg(long, default_value = "Q4_K_M")]
    quant: String,
    /// Treat the model as Mixture-of-Experts (enables expert offloading).
    #[arg(long)]
    moe: bool,
    /// Context length to plan the KV cache for.
    #[arg(long, default_value_t = 4096)]
    context: u32,
    /// Transformer layer count (0 = estimate from parameter scale).
    #[arg(long, default_value_t = 0)]
    layers: u32,
}

#[derive(clap::Args)]
struct PlanArgs {
    /// Model name or path (used only for labeling the plan).
    model: String,
    /// Plan for training instead of inference.
    #[arg(long)]
    train: bool,
    /// Training entry point to show in the previewed command.
    #[arg(long, value_name = "FILE")]
    script: Option<PathBuf>,
    #[command(flatten)]
    model_opts: ModelOpts,
}

#[derive(clap::Args)]
struct RunArgs {
    /// Model name or path.
    model: String,
    /// Force a backend: `vulkan` or `rocm`. Defaults to auto (by tier).
    #[arg(long)]
    backend: Option<BackendArg>,
    #[command(flatten)]
    model_opts: ModelOpts,
}

/// CLI-facing backend choice, kept separate from [`Backend`] so `cameo-config`
/// needs no clap dependency.
#[derive(Clone, Copy, clap::ValueEnum)]
enum BackendArg {
    Auto,
    Vulkan,
    Rocm,
    /// CPU only — run the model in system RAM, no GPU. Works on any machine.
    Cpu,
}

impl From<BackendArg> for Backend {
    fn from(b: BackendArg) -> Self {
        match b {
            BackendArg::Auto => Backend::Auto,
            BackendArg::Vulkan => Backend::Vulkan,
            BackendArg::Rocm => Backend::Rocm,
            BackendArg::Cpu => Backend::Cpu,
        }
    }
}

#[derive(clap::Args)]
struct ServeArgs {
    /// Model name or path.
    model: String,
    /// Address to bind the server to. Anything but loopback needs `--api-key`.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Port to listen on.
    #[arg(long, default_value_t = 8080)]
    port: u16,
    /// Force a backend: `vulkan` or `rocm`. Defaults to auto (by tier).
    #[arg(long)]
    backend: Option<BackendArg>,
    #[command(flatten)]
    model_opts: ModelOpts,
}

#[derive(clap::Args)]
struct PullArgs {
    /// Model to fetch: an alias, a https:// URL, or owner/repo:file.gguf.
    /// Omit with --list to see available aliases and the cache contents.
    #[arg(required_unless_present = "list")]
    model: Option<String>,
    /// List the built-in aliases and what is already cached, then exit.
    #[arg(long)]
    list: bool,
}

#[derive(clap::Args)]
struct ModelArgs {
    #[command(subcommand)]
    action: ModelAction,
}

#[derive(Subcommand)]
enum ModelAction {
    /// List cached models with their sizes.
    Ls,
    /// Total disk used by the model cache.
    Du,
    /// Remove a cached model by name, alias, or filename.
    Rm {
        /// Model to remove (as shown by `cameo model ls`).
        name: String,
    },
    /// Remove interrupted `.part` downloads left by a cancelled pull.
    Gc,
}

#[derive(clap::Args)]
struct QuantizeArgs {
    /// Input model path.
    model: String,
    /// Output model path.
    #[arg(long)]
    out: String,
    /// Quantization level, e.g. Q4_K_M, Q5_K_M, Q8_0.
    #[arg(long)]
    level: String,
}

#[derive(clap::Args)]
struct TrainArgs {
    /// Path to the training config.
    config: String,
    /// Your training entry point, launched under `torchrun`. Cameo provides the
    /// launcher and the placement; the training loop is yours.
    #[arg(long, value_name = "FILE")]
    script: PathBuf,
    #[command(flatten)]
    model_opts: ModelOpts,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("CAMEO_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        fail(cli.json, "error", &e.to_string());
    }
}

fn run(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::GpuStatus => cmd_gpu_status(cli),
        Command::Plan(a) => cmd_plan(cli, a),
        Command::Run(a) => cmd_run(cli, a),
        Command::Serve(a) => cmd_serve(cli, a),
        Command::Pull(a) => cmd_pull(cli, a),
        Command::Quantize(a) => cmd_quantize(cli, a),
        Command::Train(a) => cmd_train(cli, a),
        Command::Model(a) => cmd_model(cli, a),
        Command::Install => cmd_install(cli),
    }
}

// ---- detection -------------------------------------------------------------

/// Detect the GPU topology (live on Linux, or from captured files on any OS).
///
/// File reading and error phrasing live here; the assembly order (per-card
/// `rocminfo` correlation, sysfs memory facts, host RAM) lives once in
/// [`cameo_gpu_detect::detect_topology`], shared with the daemon.
fn detect(cli: &Cli) -> Result<(Topology, Vec<TierAssessment>)> {
    let captures = Captures {
        lspci: read_opt(&cli.lspci_file)?,
        rocminfo: read_opt(&cli.rocminfo_file)?,
        topo: read_opt(&cli.topo_file)?,
        meminfo: read_opt(&cli.meminfo_file)?,
        gpu_mem: read_opt(&cli.gpu_mem_file)?,
    };

    // `_or_cpu`: no AMD GPU is not an error — it is the CPU-only case, a valid
    // target that runs the model in system RAM. Only a live-detection failure on
    // a non-Linux host (or a malformed capture) stops us here.
    let topo = detect_topology_or_cpu(&captures).map_err(|e| match e {
        cameo_gpu_detect::Error::UnsupportedOs => anyhow!(
            "live GPU detection needs Linux. On this host, pass captured output with \
             --lspci-file (and optionally --rocminfo-file / --topo-file / --meminfo-file)."
        ),
        other => anyhow!(other.to_string()),
    })?;

    let assessments = classify_topology(&topo, &OverrideDb::embedded());
    Ok((topo, assessments))
}

/// Read an optional capture file into its contents, preserving path context on error.
fn read_opt(path: &Option<PathBuf>) -> Result<Option<String>> {
    match path {
        Some(p) => std::fs::read_to_string(p)
            .map(Some)
            .map_err(|e| anyhow!("reading {}: {e}", p.display())),
        None => Ok(None),
    }
}

// ---- helpers ---------------------------------------------------------------

/// Resolve settings with precedence flag > file > auto (auto is empty here).
fn settings_from(cli: &Cli, backend: Option<Backend>) -> Result<Settings> {
    let flags = Settings {
        backend,
        hsa_override: cli.hsa_override.clone(),
        // Only a set flag overrides the file; `--allow-oversize` absent must not
        // stamp `Some(false)` over a config that enabled it.
        allow_oversize: cli.allow_oversize.then_some(true),
        serve_api_key: cli.api_key.clone(),
        ..Default::default()
    };
    let file = match &cli.config {
        Some(path) => Settings::load_file(path)
            .map_err(|e| anyhow!("loading config {}: {e}", path.display()))?,
        None => Settings::default(),
    };
    Ok(cameo_config::resolve(Settings::default(), file, flags))
}

fn model_meta(name: &str, o: &ModelOpts) -> ModelMeta {
    let quant = QuantLevel::parse(&o.quant).unwrap_or(QuantLevel::Q4_K_M);
    let mut m = if o.moe {
        ModelMeta::moe(name, o.params, quant)
    } else {
        ModelMeta::dense(name, o.params, quant)
    };
    m.context_len = o.context;
    if o.layers > 0 {
        m.n_layers = o.layers;
    }
    m
}

fn binary_for(backend: Backend) -> &'static str {
    match backend {
        Backend::Rocm => cameo_backend_rocm::DEFAULT_BINARY,
        _ => cameo_backend_vulkan::DEFAULT_BINARY,
    }
}

/// llama.cpp's HTTP server binary. The backend selects the build, not the name,
/// so both tiers resolve to the same program today.
const SERVER_BINARY: &str = "llama-server";

/// Whether an address reaches this machine only.
fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Print a plan (and optional command) as JSON or human text.
fn emit_plan(cli: &Cli, plan: &PlacementPlan, spec: Option<&CommandSpec>) {
    if cli.json {
        let mut obj = serde_json::json!({ "plan": plan });
        if let Some(s) = spec {
            obj["command"] = serde_json::json!({
                "program": s.program, "args": s.args, "env": s.env, "shell": s.display(),
            });
        }
        println!("{}", serde_json::to_string_pretty(&obj).unwrap());
        return;
    }
    println!("Task:     {:?}", plan.task);
    println!("Backend:  {:?}", plan.backend);
    println!("GPUs:     {}", plan.gpu_count);
    println!("Strategy: {:?}", plan.multi_gpu);
    println!(
        "Offload:  layers={:?} experts_on_host={} kv_on_host={}",
        plan.offload.gpu_layers, plan.offload.experts_on_host, plan.offload.kv_on_host
    );
    let fit_label = if plan.backend == Backend::Cpu {
        "Fits RAM: "
    } else {
        "Fits VRAM:"
    };
    println!(
        "{fit_label}{}",
        if plan.fits_in_vram { " yes" } else { " no" }
    );
    for n in &plan.notes {
        println!("  • {n}");
    }
    if let Some(s) = spec {
        println!("\nCommand:\n  {}", s.display());
    }
}

/// Either print the command (dry-run) or execute it through the chosen backend.
fn run_or_dry(cli: &Cli, plan: &PlacementPlan, spec: &CommandSpec) -> Result<()> {
    if cli.dry_run {
        emit_plan(cli, plan, Some(spec));
        return Ok(());
    }
    let res = match plan.backend {
        Backend::Rocm => cameo_backend_rocm::run(spec),
        _ => cameo_backend_vulkan::run(spec),
    };
    match res {
        Ok(()) => Ok(()),
        Err(e) => fail(cli.json, "exec_error", &e.to_string()),
    }
}

// ---- commands --------------------------------------------------------------

fn cmd_gpu_status(cli: &Cli) -> Result<()> {
    if !cli.json {
        println!("{}", style::banner());
    }
    let (topo, assessments) = detect(cli)?;

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "gpus": assessments,
                "host_mem": topo.host_mem,
                "links": topo.links.iter().map(|l| serde_json::json!({
                    "a": l.a, "b": l.b, "kind": format!("{:?}", l.kind),
                })).collect::<Vec<_>>(),
                "bottleneck": topo.bottleneck_link().map(|k| format!("{k:?}")),
            }))?
        );
        return Ok(());
    }

    if assessments.is_empty() {
        println!("{}", style::bold("No AMD GPU detected — CPU-only mode"));
        println!(
            "  {}  models run in system RAM on the CPU (any x86-64 CPU works).",
            style::dim("mode")
        );
        println!(
            "  {}  expect slower inference than a GPU; force it anywhere with --backend cpu.",
            style::dim("note")
        );
        if let Some(h) = topo.host_mem {
            println!(
                "  {}  {:.1} GiB total, {:.1} GiB available",
                style::dim("ram "),
                cameo_placement::gib(h.total_bytes),
                cameo_placement::gib(h.available_bytes)
            );
        }
        return Ok(());
    }

    for (i, a) in assessments.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let n = a.tier.as_number();
        println!(
            "{}  {}",
            style::bold(&format!("GPU {i}")),
            style::accent(&a.gpu.model)
        );
        let vram = a
            .gpu
            .vram_mb
            .map(|m| format!("{m} MiB"))
            .unwrap_or_else(|| "unknown".into());
        let arch = a
            .gpu
            .gfx_arch
            .as_deref()
            .unwrap_or("unknown (no ROCm stack)");
        let train = if a.training_supported {
            style::tier(1, "train ✓")
        } else {
            style::dim("no training")
        };
        println!("  {}  {}", style::dim("pci "), a.gpu.pci_id);
        println!("  {}  {}", style::dim("vram"), vram);
        if a.gpu.memory.is_shared_with_host() {
            let gtt = a
                .gpu
                .gtt_mb
                .map(|m| format!(", {m} MiB GTT"))
                .unwrap_or_default();
            println!(
                "  {}  shared with system RAM{}",
                style::dim("mem "),
                style::dim(&gtt)
            );
        }
        println!("  {}  {}", style::dim("arch"), arch);
        println!(
            "  {}  {}   {}",
            style::dim("tier"),
            style::tier(n, &format!("● Tier {n}")),
            train
        );
        if let Some(o) = &a.hsa_override {
            println!("  {}  HSA_OVERRIDE_GFX_VERSION={o}", style::dim("ovr "));
        }
        println!("  {}  {}", style::dim("why "), style::dim(&a.rationale));
    }

    if let Some(h) = topo.host_mem {
        println!(
            "\n{}  {:.1} GiB total, {:.1} GiB available",
            style::bold("Host RAM"),
            cameo_placement::gib(h.total_bytes),
            cameo_placement::gib(h.available_bytes)
        );
    }

    if topo.is_multi_gpu() {
        println!("\n{}", style::bold("Topology"));
        for l in &topo.links {
            println!("  GPU{} ←→ GPU{}  {:?}", l.a, l.b, l.kind);
        }
        if let Some(b) = topo.bottleneck_link() {
            println!("  {} {b:?}", style::dim("bottleneck"));
        }
    }
    Ok(())
}

fn cmd_plan(cli: &Cli, args: &PlanArgs) -> Result<()> {
    let (topo, assessments) = detect(cli)?;
    let model = model_meta(&args.model, &args.model_opts);
    let task = if args.train {
        Task::Training
    } else {
        Task::Inference
    };
    let settings = settings_from(cli, None)?;

    let plan =
        make_plan(&topo, &assessments, &model, task, &settings).map_err(|e| plan_error(cli, e))?;

    let spec = match task {
        Task::Inference => build_llama_run(&plan, &model, &args.model, binary_for(plan.backend)),
        Task::Training => {
            let script = args
                .script
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "<your-train-script.py>".to_string());
            build_training(&plan, &script, "<config>")
        }
    };
    emit_plan(cli, &plan, Some(&spec));
    Ok(())
}

fn cmd_run(cli: &Cli, args: &RunArgs) -> Result<()> {
    let (topo, assessments) = detect(cli)?;
    let model = model_meta(&args.model, &args.model_opts);
    let settings = settings_from(cli, args.backend.map(Backend::from))?;

    let plan = make_plan(&topo, &assessments, &model, Task::Inference, &settings)
        .map_err(|e| plan_error(cli, e))?;
    // A dry run only prints the command, so keep the name as typed; a real run
    // needs a file on disk, so resolve it (and fail with a pull hint if absent).
    let model_path = resolve_model_path(cli, &args.model)?;
    let spec = build_llama_run(&plan, &model, &model_path, binary_for(plan.backend));
    run_or_dry(cli, &plan, &spec)
}

fn cmd_serve(cli: &Cli, args: &ServeArgs) -> Result<()> {
    let (topo, assessments) = detect(cli)?;
    let model = model_meta(&args.model, &args.model_opts);
    let settings = settings_from(cli, args.backend.map(Backend::from))?;

    // `llama-server` is an unauthenticated completion endpoint unless given a
    // key. Binding it to a routable address without one publishes the machine's
    // GPU to whoever can reach the port, so that combination is refused rather
    // than warned about.
    let api_key = settings.serve_api_key.clone();
    if !is_loopback(&args.host) && api_key.is_none() {
        return Err(anyhow!(
            "refusing to serve on {} without authentication. Pass --api-key (or set \
             CAMEO_API_KEY / serve_api_key in config), or bind to 127.0.0.1.",
            args.host
        ));
    }

    let plan = make_plan(&topo, &assessments, &model, Task::Inference, &settings)
        .map_err(|e| plan_error(cli, e))?;
    let model_path = resolve_model_path(cli, &args.model)?;
    let spec = build_llama_server(
        &plan,
        &model,
        &model_path,
        SERVER_BINARY,
        &args.host,
        args.port,
        api_key.as_deref(),
    );
    if !cli.dry_run {
        eprintln!(
            "cameo: serving {} on http://{}:{} ({:?} backend)",
            args.model, args.host, args.port, plan.backend
        );
    }
    run_or_dry(cli, &plan, &spec)
}

/// Resolve a model name to a real path for execution. A real run must resolve
/// to a `.gguf` on disk. A `--dry-run` still resolves when the file is present
/// — so the printed command matches what would execute — but falls back to the
/// name when it is absent, keeping dry-run usable as a planning aid pre-download.
fn resolve_model_path(cli: &Cli, name: &str) -> Result<String> {
    match cameo_models::resolve(name) {
        Ok(path) => Ok(path),
        Err(_) if cli.dry_run => Ok(name.to_string()),
        Err(e) => Err(e),
    }
}

/// Print the built-in alias table and the current cache contents.
fn list_models() -> Result<()> {
    println!("Aliases (cameo pull <name>):");
    for a in cameo_models::aliases() {
        println!("  {:<14} {}", a.name, a.repo);
    }
    println!("\nCache: {}", cameo_models::models_dir().display());
    let cached = cameo_models::cached_models();
    if cached.is_empty() {
        println!("  (empty)");
    } else {
        for name in cached {
            println!("  {name}");
        }
    }
    Ok(())
}

/// Bytes as a human-readable size (B / KiB / MiB / GiB).
fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// `cameo model ls|du|rm|gc` — the local model-cache management surface (F12).
fn cmd_model(cli: &Cli, args: &ModelArgs) -> Result<()> {
    match &args.action {
        ModelAction::Ls => {
            let sizes = cameo_models::model_sizes();
            if cli.json {
                let models: Vec<_> = sizes
                    .iter()
                    .map(|(n, s)| serde_json::json!({ "name": n, "bytes": s }))
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "dir": cameo_models::models_dir().to_string_lossy(),
                        "models": models,
                    }))?
                );
            } else {
                println!("Cache: {}", cameo_models::models_dir().display());
                if sizes.is_empty() {
                    println!("  (empty)");
                }
                for (name, bytes) in &sizes {
                    println!("  {:<40} {}", name, human_bytes(*bytes));
                }
            }
        }
        ModelAction::Du => {
            let total = cameo_models::cache_bytes();
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "bytes": total }))?
                );
            } else {
                println!(
                    "{} in {}",
                    human_bytes(total),
                    cameo_models::models_dir().display()
                );
            }
        }
        ModelAction::Rm { name } => {
            let path = cameo_models::remove(name)?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({ "removed": path.to_string_lossy() })
                    )?
                );
            } else {
                eprintln!("cameo: removed {}", path.display());
            }
        }
        ModelAction::Gc => {
            let cleaned = cameo_models::gc_partials()?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "cleaned": cleaned }))?
                );
            } else if cleaned.is_empty() {
                println!("nothing to clean");
            } else {
                for c in &cleaned {
                    println!("removed {c}");
                }
            }
        }
    }
    Ok(())
}

fn cmd_pull(cli: &Cli, args: &PullArgs) -> Result<()> {
    if args.list {
        return list_models();
    }
    let spec = args
        .model
        .as_deref()
        .expect("clap requires model unless --list");
    // The CLI surfaces pull progress on stderr, matching its other status lines.
    let path = cameo_models::pull(spec, &mut |line| eprintln!("cameo: {line}"))?;
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "pulled": spec,
                "path": path.to_string_lossy(),
            }))?
        );
    }
    Ok(())
}

fn cmd_train(cli: &Cli, args: &TrainArgs) -> Result<()> {
    let (topo, assessments) = detect(cli)?;
    let model = model_meta("train-target", &args.model_opts);
    let settings = settings_from(cli, None)?;

    // Cameo launches training; it does not supply the training loop. Check the
    // script exists before planning, so the failure names the missing file
    // rather than surfacing as a torchrun error minutes later.
    if !cli.dry_run && !args.script.is_file() {
        return Err(anyhow!(
            "training script {} not found. Point --script at your entry point; \
             Cameo provides the torchrun launcher and the placement, not the loop.",
            args.script.display()
        ));
    }

    let plan = make_plan(&topo, &assessments, &model, Task::Training, &settings)
        .map_err(|e| plan_error(cli, e))?;
    let spec = build_training(&plan, &args.script.to_string_lossy(), &args.config);
    run_or_dry(cli, &plan, &spec)
}

fn cmd_quantize(cli: &Cli, args: &QuantizeArgs) -> Result<()> {
    let spec = build_quantize(&args.model, &args.out, &args.level);
    if cli.dry_run {
        if cli.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "command": { "program": spec.program, "args": spec.args, "shell": spec.display() }
                }))?
            );
        } else {
            println!("Command:\n  {}", spec.display());
        }
        return Ok(());
    }
    match cameo_quant_tools::quantize(&args.model, &args.out, &args.level) {
        Ok(()) => Ok(()),
        Err(e) => fail(cli.json, "exec_error", &e.to_string()),
    }
}

fn cmd_install(cli: &Cli) -> Result<()> {
    let (_topo, assessments) = detect(cli)?;

    // No AMD GPU → a CPU-only install: none of the GPU stack, just the CPU
    // inference engine. This is what makes Cameo install-and-run on any machine.
    if assessments.is_empty() {
        let packages = ["linux (kernel + headers)", "llama.cpp (CPU backend)"];
        if cli.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "backend": "cpu",
                    "hsa_override": serde_json::Value::Null,
                    "packages": packages,
                }))?
            );
            return Ok(());
        }
        println!("Install plan (no AMD GPU — CPU-only):");
        for p in packages {
            println!("  - {p}");
        }
        println!("\nModels run in system RAM. Any x86-64 CPU works — no GPU required.");
        return Ok(());
    }

    let top = &assessments[0];
    let mut packages = vec![
        "linux (kernel + headers)",
        "amdgpu driver",
        "mesa + vulkan-radeon (Vulkan userspace)",
        "llama.cpp (Vulkan backend)",
    ];
    if top.tier.training_supported() {
        packages.push("rocm (pinned, per tier)");
        packages.push("llama.cpp (ROCm backend)");
        packages.push("python-pytorch-rocm");
    }

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "tier": top.tier.as_number(),
                "hsa_override": top.hsa_override,
                "packages": packages,
            }))?
        );
        return Ok(());
    }

    println!(
        "Install plan for {} (Tier {}):",
        top.gpu.model,
        top.tier.as_number()
    );
    if let Some(o) = &top.hsa_override {
        println!("  export HSA_OVERRIDE_GFX_VERSION={o}");
    }
    for p in packages {
        println!("  - {p}");
    }
    println!(
        "\nNote: package set and pinned versions are validated by scripts/phase1 on real hardware."
    );
    Ok(())
}

// ---- errors ----------------------------------------------------------------

/// Convert a placement error into a clean CLI exit (never returns for known cases).
fn plan_error(cli: &Cli, e: cameo_placement::Error) -> anyhow::Error {
    match e {
        cameo_placement::Error::TrainingUnsupported(tier) => fail(
            cli.json,
            "tier_unsupported",
            &format!("training requires a Tier 1/2 (ROCm) GPU; top GPU is Tier {tier}"),
        ),
        e @ cameo_placement::Error::InsufficientMemory { .. } => {
            fail(cli.json, "insufficient_memory", &e.to_string())
        }
        e @ cameo_placement::Error::InvalidModel(_) => {
            fail(cli.json, "invalid_model", &e.to_string())
        }
        other => anyhow!(other.to_string()),
    }
}

/// Emit an error (JSON or plain) and exit with a code derived from `code`.
fn fail(json: bool, code: &str, message: &str) -> ! {
    if json {
        println!(
            "{}",
            serde_json::json!({"status": "error", "code": code, "message": message})
        );
    } else {
        eprintln!("cameo: {message}");
    }
    exit(match code {
        "tier_unsupported" => 2,
        "exec_error" => 3,
        "insufficient_memory" => 4,
        "invalid_model" => 5,
        _ => 1,
    });
}
