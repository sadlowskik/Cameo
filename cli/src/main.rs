//! `cameo` — the command-line client for Cameo.
//!
//! Thin client: it detects the GPU topology, classifies each card's tier, runs
//! the placement engine to decide how to place work, and turns that plan into an
//! exact command line. Only the final spawn touches hardware — so `--dry-run`
//! exercises the entire brain on any OS. All commands support `--json`.
//!
//! On non-Linux dev machines, live detection is unavailable; pass captured text
//! with `--lspci-file` / `--rocminfo-file` / `--topo-file` to exercise everything.

use std::path::PathBuf;
use std::process::exit;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};

use cameo_config::{Backend, Settings};
use cameo_gpu_detect::{classify_topology, parse, topology, OverrideDb, TierAssessment, Topology};
use cameo_placement::command::{
    build_llama_run, build_llama_server, build_quantize, build_training,
};
use cameo_placement::{plan as make_plan, CommandSpec, ModelMeta, PlacementPlan, QuantLevel, Task};

mod models;

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
            std::env::var_os("NO_COLOR").is_none()
                && std::env::var_os("CAMEO_NO_COLOR").is_none()
                && std::io::stdout().is_terminal()
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

    /// Read `lspci -nn` output from a file instead of the live system (dev/testing).
    #[arg(long, global = true, value_name = "FILE")]
    lspci_file: Option<PathBuf>,

    /// Read `rocminfo` output from a file instead of the live system (dev/testing).
    #[arg(long, global = true, value_name = "FILE")]
    rocminfo_file: Option<PathBuf>,

    /// Read `rocm-smi --showtopo` output from a file (multi-GPU dev/testing).
    #[arg(long, global = true, value_name = "FILE")]
    topo_file: Option<PathBuf>,

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
}

impl From<BackendArg> for Backend {
    fn from(b: BackendArg) -> Self {
        match b {
            BackendArg::Auto => Backend::Auto,
            BackendArg::Vulkan => Backend::Vulkan,
            BackendArg::Rocm => Backend::Rocm,
        }
    }
}

#[derive(clap::Args)]
struct ServeArgs {
    /// Model name or path.
    model: String,
    /// Address to bind the server to.
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
        Command::Install => cmd_install(cli),
    }
}

// ---- detection -------------------------------------------------------------

/// Detect the GPU topology (live on Linux, or from captured files on any OS).
fn detect(cli: &Cli) -> Result<(Topology, Vec<TierAssessment>)> {
    let db = OverrideDb::embedded();
    let topo = if let Some(lspci_path) = &cli.lspci_file {
        let lspci_txt = read(lspci_path)?;
        let mut gpus = parse::parse_lspci(&lspci_txt);
        if let Some(rp) = &cli.rocminfo_file {
            if let Some(gfx) = parse::parse_rocminfo_gfx(&read(rp)?) {
                for g in &mut gpus {
                    g.gfx_arch.get_or_insert_with(|| gfx.clone());
                }
            }
        }
        let links = match &cli.topo_file {
            Some(tp) => topology::parse_rocm_smi_topo(&read(tp)?),
            None => Vec::new(),
        };
        Topology::new(gpus, links)
    } else {
        cameo_gpu_detect::collect_topology().map_err(|e| match e {
            cameo_gpu_detect::Error::UnsupportedOs => anyhow!(
                "live GPU detection needs Linux. On this host, pass captured output with \
                 --lspci-file (and optionally --rocminfo-file / --topo-file)."
            ),
            other => anyhow!(other.to_string()),
        })?
    };

    if topo.gpus.is_empty() {
        return Err(anyhow!("no AMD GPU detected"));
    }
    let assessments = classify_topology(&topo, &db);
    Ok((topo, assessments))
}

fn read(path: &PathBuf) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| anyhow!("reading {}: {e}", path.display()))
}

// ---- helpers ---------------------------------------------------------------

/// Resolve settings with precedence flag > file > auto (auto is empty here).
fn settings_from(cli: &Cli, backend: Option<Backend>) -> Result<Settings> {
    let flags = Settings {
        backend,
        hsa_override: cli.hsa_override.clone(),
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
    println!(
        "Fits VRAM:{}",
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
                "links": topo.links.iter().map(|l| serde_json::json!({
                    "a": l.a, "b": l.b, "kind": format!("{:?}", l.kind),
                })).collect::<Vec<_>>(),
                "bottleneck": topo.bottleneck_link().map(|k| format!("{k:?}")),
            }))?
        );
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
        Task::Inference => Some(build_llama_run(
            &plan,
            &model,
            &args.model,
            binary_for(plan.backend),
        )),
        Task::Training => Some(build_training(&plan, "<config>")),
    };
    emit_plan(cli, &plan, spec.as_ref());
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
    match models::resolve(name) {
        Ok(path) => Ok(path),
        Err(_) if cli.dry_run => Ok(name.to_string()),
        Err(e) => Err(e),
    }
}

fn cmd_pull(cli: &Cli, args: &PullArgs) -> Result<()> {
    if args.list {
        return models::list();
    }
    let spec = args
        .model
        .as_deref()
        .expect("clap requires model unless --list");
    let path = models::pull(spec)?;
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

    let plan = make_plan(&topo, &assessments, &model, Task::Training, &settings)
        .map_err(|e| plan_error(cli, e))?;
    let spec = build_training(&plan, &args.config);
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
        _ => 1,
    });
}
