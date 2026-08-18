//! Model acquisition and resolution.
//!
//! The CLI spawns `llama-server -m <path>`; without a real path that spawn
//! fails with ENOENT. This module turns a friendly name into a local `.gguf`
//! path and provides `cameo pull` to fetch one into a cache.
//!
//! Downloads shell out to `curl`, which is already in the image and matches
//! the project's execution-boundary pattern (the CLI never links an HTTP
//! stack; it drives external tools). Nothing here runs during `--dry-run`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Result};

/// A curated alias → (HuggingFace repo, file) table. Weighted toward models
/// that fit a 4 GB Tier-3 APU, since that is Cameo's proving-ground hardware.
/// Filenames verified against the HuggingFace model API.
const ALIASES: &[(&str, &str, &str)] = &[
    (
        "qwen2.5-0.5b",
        "bartowski/Qwen2.5-0.5B-Instruct-GGUF",
        "Qwen2.5-0.5B-Instruct-Q4_K_M.gguf",
    ),
    (
        "tinyllama",
        "TheBloke/TinyLlama-1.1B-Chat-v1.0-GGUF",
        "tinyllama-1.1b-chat-v1.0.Q4_K_M.gguf",
    ),
    (
        "llama3.2-3b",
        "bartowski/Llama-3.2-3B-Instruct-GGUF",
        "Llama-3.2-3B-Instruct-Q4_K_M.gguf",
    ),
];

/// Where pulled models live: `$CAMEO_MODELS_DIR`, else `$HOME/.cache/cameo/models`.
/// On the live image root's home is `/root`, so `/root/.cache/cameo/models`.
pub fn models_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CAMEO_MODELS_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cache/cameo/models")
}

/// Resolve a model argument to a path to hand `llama.cpp` `-m`.
///
/// - An existing file, or anything that looks like a path (has a separator or
///   a `.gguf` suffix), is passed through untouched — people with their own
///   GGUF keep working exactly as before.
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
/// Accepted forms: a curated alias, a full `http(s)://` URL, or a
/// HuggingFace `owner/repo:file.gguf` reference.
fn spec_to_url(spec: &str) -> Result<(String, String)> {
    if let Some((_, repo, file)) = ALIASES.iter().find(|(a, _, _)| *a == spec) {
        let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
        // Save under the alias so `cameo serve <alias>` resolves predictably.
        return Ok((url, format!("{spec}.gguf")));
    }

    if spec.starts_with("https://") || spec.starts_with("http://") {
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

/// Print the alias table and the current cache contents.
pub fn list() -> Result<()> {
    println!("Aliases (cameo pull <name>):");
    for (alias, repo, _) in ALIASES {
        println!("  {alias:<14} {repo}");
    }
    let dir = models_dir();
    println!("\nCache: {}", dir.display());
    match std::fs::read_dir(&dir) {
        Ok(entries) => {
            let mut found = false;
            for e in entries.flatten() {
                if e.path().extension().is_some_and(|x| x == "gguf") {
                    println!("  {}", e.file_name().to_string_lossy());
                    found = true;
                }
            }
            if !found {
                println!("  (empty)");
            }
        }
        Err(_) => println!("  (not created yet)"),
    }
    Ok(())
}

/// Download a model into the cache, resuming a partial file if present.
/// Returns the final path. Writes to a `.part` sidecar and renames on success
/// so an interrupted pull never leaves a truncated file that looks complete.
pub fn pull(spec: &str) -> Result<PathBuf> {
    let (url, filename) = spec_to_url(spec)?;
    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("creating {}: {e}", dir.display()))?;

    let dest = dir.join(&filename);
    if dest.is_file() {
        eprintln!("cameo: {} already present at {}", spec, dest.display());
        return Ok(dest);
    }
    let part = dir.join(format!("{filename}.part"));

    eprintln!(
        "cameo: pulling {spec}\n  from {url}\n  to   {}",
        dest.display()
    );
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
    eprintln!("cameo: saved {}", dest.display());
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
}
