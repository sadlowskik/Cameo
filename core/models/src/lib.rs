//! Model acquisition and resolution.
//!
//! Cameo spawns `llama-server -m <path>`; without a real path that spawn fails
//! with ENOENT. This crate turns a friendly name into a local `.gguf` path and
//! fetches one into a cache. It is shared by both front ends — the `cameo` CLI
//! (`cameo pull`) and the `cameod` control plane (the dashboard's model list) —
//! so there is one cache layout and one alias table, not two.
//!
//! Downloads shell out to `curl`, which is already in the image and matches the
//! project's execution-boundary pattern: the code never links an HTTP stack, it
//! drives external tools.
//!
//! This crate returns data and never prints; presentation (human tables, JSON)
//! belongs to the caller.

use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

/// A curated alias → (HuggingFace repo, file) table. Weighted toward models that
/// fit a 4 GB Tier-3 APU, since that is Cameo's proving-ground hardware.
/// Filenames verified against the HuggingFace model API.
/// (alias, HuggingFace repo, filename, params in billions, SHA-256). The digest
/// is the publisher's LFS/Xet object SHA-256 and makes aliases reproducible even
/// though their human-facing URL uses a branch name.
const ALIASES: &[(&str, &str, &str, f64, &str)] = &[
    (
        "qwen2.5-0.5b",
        "bartowski/Qwen2.5-0.5B-Instruct-GGUF",
        "Qwen2.5-0.5B-Instruct-Q4_K_M.gguf",
        0.5,
        "6eb923e7d26e9cea28811e1a8e852009b21242fb157b26149d3b188f3a8c8653",
    ),
    (
        "tinyllama",
        "TheBloke/TinyLlama-1.1B-Chat-v1.0-GGUF",
        "tinyllama-1.1b-chat-v1.0.Q4_K_M.gguf",
        1.1,
        "9fecc3b3cd76bba89d504f29b616eedf7da85b96540e490ca5824d3f7d2776a0",
    ),
    (
        "llama3.2-3b",
        "bartowski/Llama-3.2-3B-Instruct-GGUF",
        "Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        3.0,
        "6c1a2b41161032677be168d354123594c0e6e67d2b9227c84f296ad037c728ff",
    ),
];

/// What the local model will primarily do. This is deliberately small and
/// stable; aliases can gain better scores without changing the CLI contract.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Workload {
    Chat,
    Coding,
    #[default]
    Agent,
}

impl Workload {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "chat" => Some(Self::Chat),
            "coding" | "code" => Some(Self::Coding),
            "agent" | "agents" | "knossos" => Some(Self::Agent),
            _ => None,
        }
    }
}

/// Sanitized capacity facts consumed by the pure recommender. It never probes
/// hardware itself, which keeps recommendations reproducible in tests and lets
/// both the CLI and daemon use exactly the same policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareCapacity {
    /// Accelerator memory available to a model after platform headroom.
    pub accelerator_bytes: u64,
    /// Host memory Cameo may safely spend after OS headroom.
    pub host_bytes: u64,
    pub accelerator_available: bool,
}

/// One evidence-bounded recommendation. `estimated_runtime_bytes` includes a
/// conservative 25% runtime/KV margin over the quantized weight estimate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRecommendation {
    pub workload: Workload,
    pub model: String,
    pub quant: String,
    pub context_tokens: u32,
    pub estimated_runtime_bytes: u64,
    pub fit: RecommendationFit,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationFit {
    Accelerator,
    HostOffload,
    ConservativeFallback,
}

struct RecommendationProfile {
    alias: &'static str,
    context_tokens: u32,
    chat_score: u8,
    coding_score: u8,
    agent_score: u8,
}

const RECOMMENDATION_PROFILES: &[RecommendationProfile] = &[
    RecommendationProfile {
        alias: "qwen2.5-0.5b",
        context_tokens: 16_384,
        chat_score: 45,
        coding_score: 58,
        agent_score: 55,
    },
    RecommendationProfile {
        alias: "tinyllama",
        context_tokens: 1_638,
        chat_score: 40,
        coding_score: 30,
        agent_score: 32,
    },
    RecommendationProfile {
        alias: "llama3.2-3b",
        context_tokens: 32_768,
        chat_score: 88,
        coding_score: 68,
        agent_score: 76,
    },
];

fn profile_score(profile: &RecommendationProfile, workload: Workload) -> u8 {
    match workload {
        Workload::Chat => profile.chat_score,
        Workload::Coding => profile.coding_score,
        Workload::Agent => profile.agent_score,
    }
}

fn estimated_runtime_bytes(alias: &str) -> u64 {
    // Curated aliases currently use Q4_K_M. 4.85 effective bits/weight, plus a
    // 25% runtime/KV margin. Saturating conversion protects future large entries.
    let params = params_b_for(alias).unwrap_or(0.5);
    let bytes = params * 1_000_000_000.0 * (4.85 / 8.0) * 1.25;
    bytes.min(u64::MAX as f64) as u64
}

/// Choose the highest workload-quality checksum-pinned alias that fits. A full
/// accelerator fit beats host offload; within a fit class, workload score wins,
/// then larger context, then alias for deterministic ties.
pub fn recommend(capacity: HardwareCapacity, workload: Workload) -> ModelRecommendation {
    let host_pool = capacity.host_bytes.saturating_mul(3) / 4;
    let mut ranked: Vec<(&RecommendationProfile, RecommendationFit, u64)> = RECOMMENDATION_PROFILES
        .iter()
        // Profiles may only reference pinned aliases. If a future edit forgets
        // the catalog entry, it cannot escape as a recommendation.
        .filter(|profile| aliases().iter().any(|alias| alias.name == profile.alias))
        .map(|profile| {
            let need = estimated_runtime_bytes(profile.alias);
            let fit = if capacity.accelerator_available && need <= capacity.accelerator_bytes {
                RecommendationFit::Accelerator
            } else if need <= host_pool {
                RecommendationFit::HostOffload
            } else {
                RecommendationFit::ConservativeFallback
            };
            (profile, fit, need)
        })
        .collect();
    ranked.sort_by(|(a, a_fit, a_need), (b, b_fit, b_need)| {
        let fit_rank = |fit: RecommendationFit| match fit {
            RecommendationFit::Accelerator => 0,
            RecommendationFit::HostOffload => 1,
            RecommendationFit::ConservativeFallback => 2,
        };
        fit_rank(*a_fit)
            .cmp(&fit_rank(*b_fit))
            .then_with(|| {
                if *a_fit == RecommendationFit::ConservativeFallback
                    && *b_fit == RecommendationFit::ConservativeFallback
                {
                    a_need.cmp(b_need)
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| profile_score(b, workload).cmp(&profile_score(a, workload)))
            .then_with(|| b.context_tokens.cmp(&a.context_tokens))
            .then_with(|| a.alias.cmp(b.alias))
    });
    let (profile, fit, need) = ranked
        .into_iter()
        .next()
        .expect("the curated recommendation catalog is non-empty");
    let reason = match fit {
        RecommendationFit::Accelerator => {
            format!("best pinned {workload:?} profile that fits accelerator headroom")
        }
        RecommendationFit::HostOffload => {
            format!("best pinned {workload:?} profile using bounded host offload")
        }
        RecommendationFit::ConservativeFallback => {
            "no pinned profile fits reported headroom; smallest safe catalog fallback".to_string()
        }
    };
    ModelRecommendation {
        workload,
        model: profile.alias.into(),
        quant: "Q4_K_M".into(),
        context_tokens: profile.context_tokens,
        estimated_runtime_bytes: need,
        fit,
        reason,
    }
}

/// A built-in model alias: a short name and the HuggingFace source it maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alias {
    /// The name a user types (`cameo pull <name>`, or the dashboard model list).
    pub name: &'static str,
    /// The HuggingFace `owner/repo` the file is fetched from.
    pub repo: &'static str,
    /// The GGUF filename within that repo.
    pub file: &'static str,
    /// Expected lowercase SHA-256 of the GGUF payload.
    pub sha256: &'static str,
}

/// The built-in alias table, as structured data for a caller to render.
pub fn aliases() -> Vec<Alias> {
    ALIASES
        .iter()
        .map(|(name, repo, file, _, sha256)| Alias {
            name,
            repo,
            file,
            sha256,
        })
        .collect()
}

/// Parameter count (billions) for a built-in alias, including a trailing `.gguf`.
/// Used when the caller did not pass `--params` / a JSON `params` field, so the
/// starter (`qwen2.5-0.5b`) is not planned as if it were 7B.
pub fn params_b_for(name: &str) -> Option<f64> {
    let key = name.strip_suffix(".gguf").unwrap_or(name);
    ALIASES
        .iter()
        .find(|(n, _, _, _, _)| *n == key)
        .map(|(_, _, _, p, _)| *p)
}

/// Inference-relevant architecture metadata for curated models. These values
/// mirror the publishers' model configs and let placement size KV memory before
/// llama.cpp has loaded the GGUF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InferenceMeta {
    pub native_context: u32,
    pub layers: u32,
    pub kv_heads: u32,
    pub head_dim: u32,
}

pub fn inference_meta_for(name: &str) -> Option<InferenceMeta> {
    let lower = name.to_ascii_lowercase();
    if lower.contains("qwen2.5-0.5b") {
        Some(InferenceMeta {
            native_context: 32_768,
            layers: 24,
            kv_heads: 2,
            head_dim: 64,
        })
    } else if lower.contains("tinyllama") {
        Some(InferenceMeta {
            native_context: 2_048,
            layers: 22,
            kv_heads: 4,
            head_dim: 64,
        })
    } else if lower.contains("llama3.2-3b") || lower.contains("llama-3.2-3b") {
        Some(InferenceMeta {
            native_context: 131_072,
            layers: 28,
            kv_heads: 8,
            head_dim: 128,
        })
    } else {
        None
    }
}

/// Read the small structural subset of GGUF metadata needed for KV planning.
/// The function stops as soon as every field is known, before the large
/// tokenizer arrays present in normal GGUF files.
pub fn inspect_gguf(path: &Path) -> Result<Option<InferenceMeta>> {
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        return Ok(None);
    }
    let version = read_u32(&mut reader)?;
    if !(2..=3).contains(&version) {
        return Ok(None);
    }
    let _tensor_count = read_u64(&mut reader)?;
    let metadata_count = read_u64(&mut reader)?;
    let mut context = None;
    let mut layers = None;
    let mut heads = None;
    let mut kv_heads = None;
    let mut embedding = None;

    for _ in 0..metadata_count {
        let key = read_string(&mut reader)?;
        let ty = read_u32(&mut reader)?;
        let wanted = key.ends_with(".context_length")
            || key.ends_with(".block_count")
            || key.ends_with(".attention.head_count")
            || key.ends_with(".attention.head_count_kv")
            || key.ends_with(".embedding_length");
        let value = if wanted {
            read_integer_value(&mut reader, ty)?
        } else {
            skip_value(&mut reader, ty)?;
            None
        };
        if key.ends_with(".context_length") {
            context = value.and_then(to_u32);
        } else if key.ends_with(".block_count") {
            layers = value.and_then(to_u32);
        } else if key.ends_with(".attention.head_count_kv") {
            kv_heads = value.and_then(to_u32);
        } else if key.ends_with(".attention.head_count") {
            heads = value.and_then(to_u32);
        } else if key.ends_with(".embedding_length") {
            embedding = value.and_then(to_u32);
        }
        if let (Some(native_context), Some(layers), Some(heads), Some(kv_heads), Some(embedding)) =
            (context, layers, heads, kv_heads, embedding)
        {
            if heads > 0 && embedding % heads == 0 {
                return Ok(Some(InferenceMeta {
                    native_context,
                    layers,
                    kv_heads,
                    head_dim: embedding / heads,
                }));
            }
        }
    }
    Ok(None)
}

fn to_u32(value: u64) -> Option<u32> {
    u32::try_from(value).ok()
}

fn read_u32(reader: &mut (impl Read + ?Sized)) -> Result<u32> {
    let mut b = [0; 4];
    reader.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64(reader: &mut (impl Read + ?Sized)) -> Result<u64> {
    let mut b = [0; 8];
    reader.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn read_string(reader: &mut (impl Read + ?Sized)) -> Result<String> {
    let len = usize::try_from(read_u64(reader)?).map_err(|_| anyhow!("GGUF string too large"))?;
    if len > 16 * 1024 * 1024 {
        bail!("GGUF metadata string exceeds 16 MiB");
    }
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(Into::into)
}

fn read_integer_value(reader: &mut (impl Read + ?Sized), ty: u32) -> Result<Option<u64>> {
    let value = match ty {
        0 => read_byte(reader)? as u64,
        2 => read_fixed::<2>(reader).map(u16::from_le_bytes)? as u64,
        4 => read_u32(reader)? as u64,
        10 => read_u64(reader)?,
        _ => {
            skip_value_read(reader, ty)?;
            return Ok(None);
        }
    };
    Ok(Some(value))
}

fn read_byte(reader: &mut (impl Read + ?Sized)) -> Result<u8> {
    let mut b = [0];
    reader.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_fixed<const N: usize>(reader: &mut (impl Read + ?Sized)) -> Result<[u8; N]> {
    let mut b = [0; N];
    reader.read_exact(&mut b)?;
    Ok(b)
}

fn skip_value(reader: &mut (impl Read + Seek), ty: u32) -> Result<()> {
    match ty {
        8 => {
            let len = read_u64(reader)?;
            seek_forward(reader, len)
        }
        9 => {
            let element = read_u32(reader)?;
            let count = read_u64(reader)?;
            if let Some(width) = fixed_width(element) {
                seek_forward(reader, count.saturating_mul(width))
            } else {
                for _ in 0..count {
                    skip_value(reader, element)?;
                }
                Ok(())
            }
        }
        _ => seek_forward(
            reader,
            fixed_width(ty).ok_or_else(|| anyhow!("unknown GGUF type {ty}"))?,
        ),
    }
}

fn skip_value_read(reader: &mut (impl Read + ?Sized), ty: u32) -> Result<()> {
    let width = fixed_width(ty).ok_or_else(|| anyhow!("non-integer GGUF metadata type {ty}"))?;
    let mut remaining = width;
    let mut scratch = [0u8; 16];
    while remaining > 0 {
        let take = remaining.min(scratch.len() as u64) as usize;
        reader.read_exact(&mut scratch[..take])?;
        remaining -= take as u64;
    }
    Ok(())
}

fn fixed_width(ty: u32) -> Option<u64> {
    match ty {
        0 | 1 | 7 => Some(1),
        2 | 3 => Some(2),
        4..=6 => Some(4),
        10..=12 => Some(8),
        _ => None,
    }
}

fn seek_forward(reader: &mut impl Seek, bytes: u64) -> Result<()> {
    let offset = i64::try_from(bytes).map_err(|_| anyhow!("GGUF metadata value too large"))?;
    reader.seek(SeekFrom::Current(offset))?;
    Ok(())
}

/// Where pulled models live, in precedence order:
/// 1. `$CAMEO_MODELS_DIR` — set by first-boot when the user picks a data disk,
///    by anyone who wants an explicit location, or by the front ends when the
///    config file sets `model_dir` (they export it here at startup; a var the
///    user set themselves is never overridden). Always wins.
/// 2. `/var/lib/cameo/models` when it exists — the shared, persistent location an
///    installed system, a container volume, or first-boot provides. Matches
///    `cameo_config`'s default so the CLI and daemon never disagree.
/// 3. `$HOME/.cache/cameo/models` — the per-user fallback for an unprivileged dev
///    box where the system dir is absent.
///
/// The ordering deliberately prefers persistent storage: on a live image a bare
/// `$HOME` is `/root` on a RAM overlay, so defaulting there is exactly what let a
/// pull silently fill memory. See F2 in `docs/remediation-plan.md`.
pub fn models_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CAMEO_MODELS_DIR") {
        return PathBuf::from(d);
    }
    let system = PathBuf::from("/var/lib/cameo/models");
    if system.is_dir() {
        return system;
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cache/cameo/models")
}

/// The `.gguf` files currently in the cache, by filename (sorted). A missing
/// cache directory is not an error — it just means nothing has been pulled yet.
pub fn cached_models() -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(models_dir()) {
        Ok(entries) => entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "gguf"))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

/// Cached `.gguf` files with their byte sizes, sorted by name. The management
/// surface (`cameo model ls/du/rm/gc`) is built on these pure `*_in` helpers,
/// which take an explicit directory so they test without touching the
/// environment; the public wrappers bind them to [`models_dir`].
fn model_sizes_in(dir: &Path) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "gguf"))
            .map(|e| {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                (e.file_name().to_string_lossy().into_owned(), size)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Cached models and their sizes in bytes, for `cameo model ls`.
pub fn model_sizes() -> Vec<(String, u64)> {
    model_sizes_in(&models_dir())
}

/// Total bytes the model cache occupies, for `cameo model du`.
pub fn cache_bytes() -> u64 {
    model_sizes().iter().map(|(_, s)| s).sum()
}

fn remove_in(dir: &Path, name: &str) -> Result<PathBuf> {
    // Deletion is by name/alias/filename only — never a path. `Path::join` with
    // a separator-carrying (or absolute) name can resolve outside the cache
    // dir, and this function is reachable from the daemon's DELETE route, so
    // the claim "never a path outside the cache dir" is enforced, not assumed.
    if name.contains(['/', '\\']) || name == "." || name == ".." {
        bail!("model names may not contain path separators; see `cameo model ls` for names.");
    }
    // An alias saves as `<alias>.gguf`; a user may pass the bare name or the
    // filename. Try both, never a path outside the cache dir.
    for cand in [dir.join(name), dir.join(format!("{name}.gguf"))] {
        if cand.is_file() {
            std::fs::remove_file(&cand).map_err(|e| anyhow!("removing {}: {e}", cand.display()))?;
            return Ok(cand);
        }
    }
    Err(anyhow!(
        "no cached model matches '{name}'. See `cameo model ls` for what is cached."
    ))
}

/// Remove a cached model by name/alias/filename, returning the path removed.
pub fn remove(name: &str) -> Result<PathBuf> {
    remove_in(&models_dir(), name)
}

fn gc_partials_in(dir: &Path) -> Result<Vec<String>> {
    let mut cleaned = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(cleaned);
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "part") {
            std::fs::remove_file(&p).map_err(|err| anyhow!("removing {}: {err}", p.display()))?;
            cleaned.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    cleaned.sort();
    Ok(cleaned)
}

/// Remove interrupted `.part` downloads, returning the filenames cleaned, for
/// `cameo model gc`.
pub fn gc_partials() -> Result<Vec<String>> {
    gc_partials_in(&models_dir())
}

/// Resolve a model argument to a path to hand `llama.cpp` `-m`.
///
/// - An existing file, or anything that looks like a path (has a separator or a
///   `.gguf` suffix), is passed through untouched — people with their own GGUF
///   keep working exactly as before.
/// - A bare name is looked up in the cache as `<name>.gguf` (or `<name>` if it
///   already carries the suffix). A miss is an error that names the fix.
pub fn resolve(name: &str) -> Result<String> {
    let looks_like_path =
        name.contains('/') || name.contains('\\') || Path::new(name).is_absolute();
    if looks_like_path || (name.ends_with(".gguf") && Path::new(name).exists()) {
        return Ok(name.to_string());
    }

    let dir = models_dir();
    for cand in [dir.join(name), dir.join(format!("{name}.gguf"))] {
        if cand.is_file() {
            return Ok(cand.to_string_lossy().into_owned());
        }
    }

    Err(anyhow!(
        "model '{name}' is not available locally. Fetch it first:\n    \
         cameo pull {name}\n  \
         or pass a path to a .gguf file. See `cameo pull --list` for aliases."
    ))
}

/// Turn a pull spec into (download URL, local filename).
///
/// Accepted forms: a curated alias, a full `http(s)://` URL, or a HuggingFace
/// `owner/repo:file.gguf` reference.
fn spec_to_url(spec: &str) -> Result<(String, String)> {
    if let Some((_, repo, file, _, _)) = ALIASES.iter().find(|(a, _, _, _, _)| *a == spec) {
        let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
        // Save under the alias so `cameo serve <alias>` resolves predictably.
        let filename = format!("{spec}.gguf");
        validate_pull_filename(&filename)?;
        return Ok((url, filename));
    }

    // Downloads enforce TLS (`curl --proto =https`), so accepting an `http://`
    // spec here just deferred the failure into an opaque curl error — and a
    // multi-GiB binary fetched in the clear is not something to quietly allow.
    if spec.starts_with("http://") {
        bail!("plain-HTTP model URLs are not supported (downloads require TLS); use https://");
    }
    if spec.starts_with("https://") {
        let file = spec
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("cannot derive a filename from URL: {spec}"))?;
        validate_pull_filename(file)?;
        return Ok((spec.to_string(), file.to_string()));
    }

    // owner/repo:file.gguf
    if let Some((repo, file)) = spec.split_once(':') {
        if repo.contains('/') && validate_pull_filename(file).is_ok() {
            let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
            return Ok((url, file.to_string()));
        }
    }

    bail!(
        "unrecognised model spec '{spec}'. Use an alias (see `cameo pull --list`), \
         a https:// URL, or owner/repo:file.gguf"
    )
}

/// A pulled model is always a single ordinary GGUF filename, never a path.
/// Explicitly reject both separator styles so the rule is identical on Unix and
/// Windows and `models_dir().join(filename)` cannot escape the cache.
fn validate_pull_filename(filename: &str) -> Result<()> {
    if filename.is_empty()
        || filename == "."
        || filename == ".."
        || filename.contains('/')
        || filename.contains('\\')
        || !filename.to_ascii_lowercase().ends_with(".gguf")
    {
        bail!("model destination must be a single .gguf filename, not a path");
    }
    let mut components = Path::new(filename).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        bail!("model destination must be a single .gguf filename, not a path");
    }
    Ok(())
}

fn pull_destination(dir: &Path, filename: &str) -> Result<PathBuf> {
    validate_pull_filename(filename)?;
    let dest = dir.join(filename);
    if dest.parent() != Some(dir) {
        bail!("model destination escapes the model cache");
    }
    Ok(dest)
}

/// GiB from a byte count, for human-facing sizes.
fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// Extra room a download needs beyond the model's own size: the `.part` sidecar
/// plus filesystem overhead. 15% is generous enough to never refuse a pull that
/// would actually have fit.
const PULL_SPACE_MARGIN: f64 = 1.15;

/// Available bytes at `dir`, via POSIX `df -Pk`. `None` when `df` is missing or
/// unparseable — the preflight then skips the check rather than false-refusing.
fn available_bytes(dir: &Path) -> Option<u64> {
    let out = Command::new("df").arg("-Pk").arg(dir).output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_df_available_kib(&String::from_utf8_lossy(&out.stdout))
        .map(|kib| kib.saturating_mul(1024))
}

/// Parse the Available column (4th field of the data row) from `df -Pk` output.
/// `-P` guarantees one physical line per filesystem, so the fields sit at fixed
/// positions: Filesystem, 1024-blocks, Used, Available, Capacity, Mounted-on.
fn parse_df_available_kib(df_output: &str) -> Option<u64> {
    df_output
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()
}

/// The remote size of the download, via a `curl` HEAD that follows redirects to
/// the CDN. Best-effort: `None` when the server omits `Content-Length`.
fn remote_size_bytes(url: &str) -> Option<u64> {
    let out = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--head",
            "--proto",
            "=https",
            "--tlsv1.2",
        ])
        .arg(url)
        .output()
        .ok()?;
    parse_content_length(&String::from_utf8_lossy(&out.stdout))
}

/// The last `Content-Length` in an HTTP header dump. A redirect chain emits one
/// header block per hop; the final block describes the real payload, so the last
/// value is the one that counts.
fn parse_content_length(headers: &str) -> Option<u64> {
    headers.lines().rev().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| v.trim().parse().ok())
            .flatten()
    })
}

/// Decide whether a pull may proceed given free space and remote size. Pure, so
/// the policy is unit-tested; the impure gathering lives in [`preflight_space`].
/// A `None` on either input means "unknown" — never block on missing data.
fn space_verdict(avail: Option<u64>, need: Option<u64>, dir: &Path, spec: &str) -> Result<()> {
    let (Some(avail), Some(need)) = (avail, need) else {
        return Ok(());
    };
    if avail >= (need as f64 * PULL_SPACE_MARGIN) as u64 {
        return Ok(());
    }
    bail!(
        "not enough space to pull '{spec}': it needs ~{:.1} GiB but {} has only ~{:.1} GiB free.\n  \
         On a live USB this usually means models are landing in RAM. Point Cameo at a real disk:\n    \
         export CAMEO_MODELS_DIR=/path/on/a/disk\n  \
         or free space, or choose a smaller model (see `cameo pull --list`).",
        gib(need),
        dir.display(),
        gib(avail)
    )
}

/// Refuse a pull that cannot fit at `dir`, with actionable guidance. A no-op when
/// either the free space or the remote size cannot be determined.
fn preflight_space(dir: &Path, url: &str, spec: &str) -> Result<()> {
    space_verdict(available_bytes(dir), remote_size_bytes(url), dir, spec)
}

/// A progress line emitted during a pull, so a caller can surface it however it
/// likes (the CLI prints it; the daemon could log it) without this crate
/// choosing an output stream.
pub type Progress<'a> = dyn FnMut(&str) + 'a;

fn normalize_sha256(value: &str) -> Result<String> {
    let digest = value.trim().to_ascii_lowercase();
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("SHA-256 must be exactly 64 hexadecimal characters");
    }
    Ok(digest)
}

fn expected_sha256(spec: &str, supplied: Option<&str>) -> Result<String> {
    let supplied = supplied.map(normalize_sha256).transpose()?;
    let curated = ALIASES
        .iter()
        .find(|(alias, _, _, _, _)| *alias == spec)
        .map(|(_, _, _, _, sha256)| (*sha256).to_string());
    if let (Some(given), Some(known)) = (&supplied, &curated) {
        if given != known {
            bail!("--sha256 conflicts with the curated digest for alias '{spec}'");
        }
    }
    supplied.or(curated).ok_or_else(|| {
        anyhow!("custom model sources require an expected digest; pass --sha256 <64-hex-digest>")
    })
}

#[cfg(test)]
fn parse_sha256_output(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|word| {
        let candidate = word.trim().to_ascii_lowercase();
        (candidate.len() == 64 && candidate.bytes().all(|b| b.is_ascii_hexdigit()))
            .then_some(candidate)
    })
}

/// Hash a regular model artifact with bounded memory and no external utility.
pub fn file_sha256(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        bail!("model integrity requires a regular file");
    }
    let mut file = std::fs::File::open(path)?;
    let before = file.metadata()?;
    if !before.is_file() {
        bail!("model integrity requires a regular file");
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let after = file.metadata()?;
    if before.len() != after.len() || before.modified()? != after.modified()? {
        bail!("model changed while checking integrity");
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Download a model into the cache, resuming a partial file if present. Returns
/// the final path. Writes to a `.part` sidecar and renames on success so an
/// interrupted pull never leaves a truncated file that looks complete.
///
/// `report` receives human-readable progress lines; pass `|_| {}` to ignore them.
pub fn pull(spec: &str, report: &mut Progress<'_>) -> Result<PathBuf> {
    pull_with_checksum(spec, None, report)
}

/// Pull a model and verify it against either the curated alias digest or the
/// caller-supplied digest. Custom URLs and repository references fail closed
/// when `sha256` is absent.
pub fn pull_with_checksum(
    spec: &str,
    sha256: Option<&str>,
    report: &mut Progress<'_>,
) -> Result<PathBuf> {
    let (url, filename) = spec_to_url(spec)?;
    let expected = expected_sha256(spec, sha256)?;
    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("creating {}: {e}", dir.display()))?;

    let dest = pull_destination(&dir, &filename)?;
    if dest.is_file() {
        let actual = file_sha256(&dest)?;
        if actual != expected {
            bail!(
                "cached model {} failed SHA-256 verification (got {actual}, want {expected})",
                dest.display()
            );
        }
        report(&format!("{} already present at {}", spec, dest.display()));
        return Ok(dest);
    }
    let part = dir.join(format!("{filename}.part"));

    // Refuse before downloading if the target cannot hold the model — otherwise a
    // live-USB pull silently fills the RAM overlay (F2, docs/remediation-plan.md).
    preflight_space(&dir, &url, spec)?;

    report(&format!(
        "pulling {spec}\n  from {url}\n  to   {}",
        dest.display()
    ));
    let status = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--continue-at",
            "-",
            "--output",
        ])
        .arg(&part)
        .arg(&url)
        .status()
        .map_err(|e| anyhow!("could not run curl (is it installed?): {e}"))?;

    if !status.success() {
        bail!(
            "download failed (curl exit {:?}); partial file kept at {}",
            status.code(),
            part.display()
        );
    }

    let actual = file_sha256(&part)?;
    if actual != expected {
        let _ = std::fs::remove_file(&part);
        bail!("SHA-256 mismatch for '{spec}' (got {actual}, want {expected})");
    }

    std::fs::rename(&part, &dest).map_err(|e| anyhow!("finalising {}: {e}", dest.display()))?;
    report(&format!("verified {expected}\nsaved {}", dest.display()));
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_sha256_matches_known_vector_and_rejects_directories() {
        let dir = std::env::temp_dir().join(format!("cameo-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("vector.gguf");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(
            file_sha256(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(file_sha256(&dir).is_err());
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn recommender_chooses_best_pinned_profile_that_fits_accelerator() {
        let rec = recommend(
            HardwareCapacity {
                accelerator_bytes: 4 * 1024 * 1024 * 1024,
                host_bytes: 8 * 1024 * 1024 * 1024,
                accelerator_available: true,
            },
            Workload::Agent,
        );
        assert_eq!(rec.model, "llama3.2-3b");
        assert_eq!(rec.fit, RecommendationFit::Accelerator);
        assert!(aliases()
            .iter()
            .any(|alias| alias.name == rec.model && alias.sha256.len() == 64));
    }

    #[test]
    fn accelerator_fit_beats_larger_host_offload_profile() {
        let rec = recommend(
            HardwareCapacity {
                accelerator_bytes: 1024 * 1024 * 1024,
                host_bytes: 16 * 1024 * 1024 * 1024,
                accelerator_available: true,
            },
            Workload::Chat,
        );
        assert_eq!(rec.model, "qwen2.5-0.5b");
        assert_eq!(rec.fit, RecommendationFit::Accelerator);
    }

    #[test]
    fn cpu_only_can_recommend_bounded_host_offload() {
        let rec = recommend(
            HardwareCapacity {
                accelerator_bytes: 0,
                host_bytes: 8 * 1024 * 1024 * 1024,
                accelerator_available: false,
            },
            Workload::Chat,
        );
        assert_eq!(rec.model, "llama3.2-3b");
        assert_eq!(rec.fit, RecommendationFit::HostOffload);
    }

    #[test]
    fn unknown_capacity_falls_back_to_smallest_profile() {
        let rec = recommend(
            HardwareCapacity {
                accelerator_bytes: 0,
                host_bytes: 0,
                accelerator_available: false,
            },
            Workload::Agent,
        );
        assert_eq!(rec.model, "qwen2.5-0.5b");
        assert_eq!(rec.fit, RecommendationFit::ConservativeFallback);
    }

    #[test]
    fn passes_through_paths() {
        assert_eq!(resolve("/models/x.gguf").unwrap(), "/models/x.gguf");
        assert_eq!(resolve("./sub/y.gguf").unwrap(), "./sub/y.gguf");
    }

    #[test]
    fn bare_name_miss_names_the_fix() {
        std::env::set_var(
            "CAMEO_MODELS_DIR",
            std::env::temp_dir().join("cameo-empty-xyz"),
        );
        let err = resolve("nonesuch-model").unwrap_err().to_string();
        assert!(err.contains("cameo pull nonesuch-model"), "got: {err}");
        std::env::remove_var("CAMEO_MODELS_DIR");
    }

    #[test]
    fn alias_maps_to_hf_resolve_url() {
        let (url, file) = spec_to_url("tinyllama").unwrap();
        assert_eq!(file, "tinyllama.gguf");
        assert!(url.starts_with("https://huggingface.co/TheBloke/"));
        assert!(url.ends_with(".gguf"));
    }

    #[test]
    fn repo_file_spec_builds_url() {
        let (url, file) = spec_to_url("bartowski/Foo-GGUF:Foo-Q4_K_M.gguf").unwrap();
        assert_eq!(file, "Foo-Q4_K_M.gguf");
        assert_eq!(
            url,
            "https://huggingface.co/bartowski/Foo-GGUF/resolve/main/Foo-Q4_K_M.gguf"
        );
    }

    #[test]
    fn pull_specs_cannot_escape_the_model_cache() {
        for bad in [
            "owner/repo:../../target.gguf",
            "owner/repo:/tmp/target.gguf",
            "owner/repo:..\\target.gguf",
            "https://example.com/path/..\\target.gguf",
        ] {
            assert!(spec_to_url(bad).is_err(), "accepted {bad}");
        }

        let cache = Path::new("cache");
        assert_eq!(
            pull_destination(cache, "model.gguf").unwrap(),
            cache.join("model.gguf")
        );
        assert!(pull_destination(cache, "../target.gguf").is_err());
    }

    #[test]
    fn bare_url_keeps_basename() {
        let (url, file) = spec_to_url("https://example.com/path/model.gguf").unwrap();
        assert_eq!(file, "model.gguf");
        assert_eq!(url, "https://example.com/path/model.gguf");
    }

    #[test]
    fn junk_spec_is_rejected() {
        assert!(spec_to_url("not a real spec").is_err());
    }

    #[test]
    fn aliases_are_exposed_as_data() {
        let a = aliases();
        assert!(a.iter().any(|x| x.name == "tinyllama"));
        assert!(a.iter().all(|x| x.file.ends_with(".gguf")));
        assert!(a.iter().all(|x| x.sha256.len() == 64));
        assert_eq!(params_b_for("qwen2.5-0.5b"), Some(0.5));
        assert_eq!(params_b_for("qwen2.5-0.5b.gguf"), Some(0.5));
        assert_eq!(params_b_for("mystery-model"), None);
    }

    #[test]
    fn custom_sources_require_a_well_formed_digest() {
        assert!(expected_sha256("https://example.com/x.gguf", None).is_err());
        assert!(expected_sha256("https://example.com/x.gguf", Some("nope")).is_err());
        let digest = "a".repeat(64);
        assert_eq!(
            expected_sha256("https://example.com/x.gguf", Some(&digest)).unwrap(),
            digest
        );
    }

    #[test]
    fn curated_digest_cannot_be_overridden() {
        let wrong = "a".repeat(64);
        assert!(expected_sha256("tinyllama", Some(&wrong)).is_err());
        assert_eq!(expected_sha256("tinyllama", None).unwrap(), ALIASES[1].4);
    }

    #[test]
    fn sha256_output_parser_accepts_common_tool_formats() {
        let digest = "0123456789abcdef".repeat(4);
        assert_eq!(
            parse_sha256_output(&format!("{digest}  model.gguf\n")),
            Some(digest.clone())
        );
        assert_eq!(
            parse_sha256_output(&format!("SHA256 hash of file:\r\n{digest}\r\nCertUtil: ok")),
            Some(digest)
        );
    }

    #[test]
    fn df_available_column_is_parsed() {
        let out = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                   /dev/sda1 100000000 40000000 60000000 40% /\n";
        assert_eq!(parse_df_available_kib(out), Some(60_000_000));
    }

    #[test]
    fn df_garbage_is_none() {
        assert_eq!(parse_df_available_kib("nonsense"), None);
        assert_eq!(parse_df_available_kib(""), None);
    }

    #[test]
    fn content_length_takes_the_last_block() {
        // A 301 redirect (length 0) then the real 200 with the payload size.
        let headers = "HTTP/1.1 301 Moved\r\ncontent-length: 0\r\n\r\n\
                       HTTP/2 200\r\nContent-Length: 4096\r\ncontent-type: application/octet-stream\r\n";
        assert_eq!(parse_content_length(headers), Some(4096));
    }

    #[test]
    fn content_length_absent_is_none() {
        assert_eq!(
            parse_content_length("HTTP/2 200\r\ncontent-type: x\r\n"),
            None
        );
    }

    #[test]
    fn space_verdict_refuses_when_too_small_with_guidance() {
        let need = 4 * 1024 * 1024 * 1024; // 4 GiB
        let avail = 1024 * 1024 * 1024; // 1 GiB
        let err = space_verdict(
            Some(avail),
            Some(need),
            Path::new("/var/lib/cameo/models"),
            "big",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("CAMEO_MODELS_DIR"), "got: {err}");
    }

    #[test]
    fn space_verdict_allows_when_it_fits() {
        let need = 1024 * 1024 * 1024;
        let avail = 4 * 1024 * 1024 * 1024;
        assert!(space_verdict(Some(avail), Some(need), Path::new("/tmp"), "small").is_ok());
    }

    #[test]
    fn space_verdict_skips_on_unknowns() {
        let d = Path::new("/tmp");
        assert!(space_verdict(None, Some(999), d, "x").is_ok());
        assert!(space_verdict(Some(10), None, d, "x").is_ok());
        assert!(space_verdict(None, None, d, "x").is_ok());
    }

    #[test]
    fn space_verdict_honours_the_margin() {
        // avail == need but not the 15% headroom → refuse; comfortably over → allow.
        let n = 1_000_000_000u64;
        let d = Path::new("/tmp");
        assert!(space_verdict(Some(n), Some(n), d, "x").is_err());
        assert!(space_verdict(Some((n as f64 * 1.2) as u64), Some(n), d, "x").is_ok());
    }

    fn fresh_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "cameo-models-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(dir: &Path, name: &str, bytes: usize) {
        std::fs::write(dir.join(name), vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn model_sizes_lists_gguf_with_sizes_sorted() {
        let d = fresh_dir();
        touch(&d, "b.gguf", 200);
        touch(&d, "a.gguf", 100);
        touch(&d, "notes.txt", 999); // ignored
        assert_eq!(
            model_sizes_in(&d),
            vec![("a.gguf".into(), 100), ("b.gguf".into(), 200)]
        );
    }

    #[test]
    fn remove_matches_bare_name_and_filename() {
        let d = fresh_dir();
        touch(&d, "tinyllama.gguf", 10);
        assert!(remove_in(&d, "tinyllama").is_ok()); // bare name → <name>.gguf
        assert!(!d.join("tinyllama.gguf").exists());
        touch(&d, "foo.gguf", 10);
        assert!(remove_in(&d, "foo.gguf").is_ok()); // explicit filename
    }

    #[test]
    fn remove_rejects_path_shaped_names() {
        let d = fresh_dir();
        touch(&d, "real.gguf", 10);
        for bad in ["../real", "..", "a/b", "a\\b", "/etc/passwd", "."] {
            let err = remove_in(&d, bad).unwrap_err().to_string();
            assert!(err.contains("path separators"), "'{bad}' got: {err}");
        }
        assert!(d.join("real.gguf").exists());
    }

    #[test]
    fn plain_http_specs_are_refused_with_guidance() {
        let err = spec_to_url("http://example.com/model.gguf")
            .unwrap_err()
            .to_string();
        assert!(err.contains("https://"), "got: {err}");
    }

    #[test]
    fn remove_missing_names_the_fix() {
        let d = fresh_dir();
        let err = remove_in(&d, "ghost").unwrap_err().to_string();
        assert!(err.contains("cameo model ls"), "got: {err}");
    }

    #[test]
    fn gc_removes_only_partials() {
        let d = fresh_dir();
        touch(&d, "keep.gguf", 5);
        touch(&d, "x.gguf.part", 5);
        touch(&d, "y.gguf.part", 5);
        assert_eq!(
            gc_partials_in(&d).unwrap(),
            vec!["x.gguf.part".to_string(), "y.gguf.part".to_string()]
        );
        assert!(d.join("keep.gguf").exists());
        assert!(!d.join("x.gguf.part").exists());
    }

    #[test]
    fn gguf_inspector_reads_native_context_and_gqa_shape() {
        fn put_string(out: &mut Vec<u8>, value: &str) {
            out.extend_from_slice(&(value.len() as u64).to_le_bytes());
            out.extend_from_slice(value.as_bytes());
        }
        fn put_u32_meta(out: &mut Vec<u8>, key: &str, value: u32) {
            put_string(out, key);
            out.extend_from_slice(&4u32.to_le_bytes());
            out.extend_from_slice(&value.to_le_bytes());
        }

        let dir = fresh_dir();
        let path = dir.join("meta.gguf");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"GGUF");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&6u64.to_le_bytes());
        put_string(&mut bytes, "general.name");
        bytes.extend_from_slice(&8u32.to_le_bytes());
        put_string(&mut bytes, "Qwen test");
        put_u32_meta(&mut bytes, "qwen2.context_length", 32_768);
        put_u32_meta(&mut bytes, "qwen2.block_count", 48);
        put_u32_meta(&mut bytes, "qwen2.attention.head_count", 40);
        put_u32_meta(&mut bytes, "qwen2.attention.head_count_kv", 8);
        put_u32_meta(&mut bytes, "qwen2.embedding_length", 5_120);
        std::fs::write(&path, bytes).unwrap();

        assert_eq!(
            inspect_gguf(&path).unwrap(),
            Some(InferenceMeta {
                native_context: 32_768,
                layers: 48,
                kv_heads: 8,
                head_dim: 128,
            })
        );
    }
}
