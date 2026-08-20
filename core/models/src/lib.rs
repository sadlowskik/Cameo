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

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Result};

/// A curated alias → (HuggingFace repo, file) table. Weighted toward models that
/// fit a 4 GB Tier-3 APU, since that is Cameo's proving-ground hardware.
/// Filenames verified against the HuggingFace model API.
/// (alias, huggingface repo, filename, params in billions). The last column
/// is what the planner uses when the caller omits `--params`.
const ALIASES: &[(&str, &str, &str, f64)] = &[
    (
        "qwen2.5-0.5b",
        "bartowski/Qwen2.5-0.5B-Instruct-GGUF",
        "Qwen2.5-0.5B-Instruct-Q4_K_M.gguf",
        0.5,
    ),
    (
        "tinyllama",
        "TheBloke/TinyLlama-1.1B-Chat-v1.0-GGUF",
        "tinyllama-1.1b-chat-v1.0.Q4_K_M.gguf",
        1.1,
    ),
    (
        "llama3.2-3b",
        "bartowski/Llama-3.2-3B-Instruct-GGUF",
        "Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        3.0,
    ),
];

/// A built-in model alias: a short name and the HuggingFace source it maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alias {
    /// The name a user types (`cameo pull <name>`, or the dashboard model list).
    pub name: &'static str,
    /// The HuggingFace `owner/repo` the file is fetched from.
    pub repo: &'static str,
    /// The GGUF filename within that repo.
    pub file: &'static str,
}

/// The built-in alias table, as structured data for a caller to render.
pub fn aliases() -> Vec<Alias> {
    ALIASES
        .iter()
        .map(|(name, repo, file, _)| Alias { name, repo, file })
        .collect()
}

/// Parameter count (billions) for a built-in alias, including a trailing `.gguf`.
/// Used when the caller did not pass `--params` / a JSON `params` field, so the
/// starter (`qwen2.5-0.5b`) is not planned as if it were 7B.
pub fn params_b_for(name: &str) -> Option<f64> {
    let key = name.strip_suffix(".gguf").unwrap_or(name);
    ALIASES
        .iter()
        .find(|(n, _, _, _)| *n == key)
        .map(|(_, _, _, p)| *p)
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
    if let Some((_, repo, file, _)) = ALIASES.iter().find(|(a, _, _, _)| *a == spec) {
        let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
        // Save under the alias so `cameo serve <alias>` resolves predictably.
        return Ok((url, format!("{spec}.gguf")));
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
        return Ok((spec.to_string(), file.to_string()));
    }

    // owner/repo:file.gguf
    if let Some((repo, file)) = spec.split_once(':') {
        if repo.contains('/') && file.ends_with(".gguf") {
            let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
            return Ok((url, file.to_string()));
        }
    }

    bail!(
        "unrecognised model spec '{spec}'. Use an alias (see `cameo pull --list`), \
         a https:// URL, or owner/repo:file.gguf"
    )
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

/// Download a model into the cache, resuming a partial file if present. Returns
/// the final path. Writes to a `.part` sidecar and renames on success so an
/// interrupted pull never leaves a truncated file that looks complete.
///
/// `report` receives human-readable progress lines; pass `|_| {}` to ignore them.
pub fn pull(spec: &str, report: &mut Progress<'_>) -> Result<PathBuf> {
    let (url, filename) = spec_to_url(spec)?;
    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("creating {}: {e}", dir.display()))?;

    let dest = dir.join(&filename);
    if dest.is_file() {
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

    std::fs::rename(&part, &dest).map_err(|e| anyhow!("finalising {}: {e}", dest.display()))?;
    report(&format!("saved {}", dest.display()));
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(params_b_for("qwen2.5-0.5b"), Some(0.5));
        assert_eq!(params_b_for("qwen2.5-0.5b.gguf"), Some(0.5));
        assert_eq!(params_b_for("mystery-model"), None);
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
}
