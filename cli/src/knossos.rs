//! `cameo knossos …` — install, inspect and remove the Knossos harness on demand.
//!
//! Knossos is not baked into the image (PRODUCTIZATION_PLAN §5, must-item 7).
//! This pulls the release named in `contracts/knossos-release.lock.json`,
//! verifies its SHA-256 against that lock, extracts it under the Cameo state
//! directory and links the binary onto PATH. The lock is embedded at compile
//! time, so what a given `cameo` will install is fixed by the commit it was
//! built from and cannot be steered by anything on the network.
//!
//! Download and verification follow `cameo pull`: `curl` pinned to HTTPS with
//! TLS 1.2+, a `.part` sidecar, digest check before the rename. Extraction
//! uses `bsdtar` (libarchive, always present on the image), never a shell.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

const LOCK: &str = include_str!("../../contracts/knossos-release.lock.json");
const DEFAULT_DIR: &str = "/var/lib/cameo/knossos";
const DEFAULT_LINK: &str = "/usr/local/bin/knossos";

/// The pinned release. One asset per platform key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Lock {
    pub schema: String,
    pub version: String,
    pub tag: String,
    pub repository: String,
    pub assets: std::collections::BTreeMap<String, Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Asset {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// Path of the executable inside the archive.
    pub binary: String,
}

#[derive(clap::Args)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Download, verify and install the pinned release; `--dry-run` prints the plan.
    Install {
        /// Where releases are kept (persistent state on an installed system).
        #[arg(long, default_value = DEFAULT_DIR, value_name = "DIR")]
        dir: PathBuf,
        /// Symlink placed on PATH; `--no-link` skips it.
        #[arg(long, default_value = DEFAULT_LINK, value_name = "PATH")]
        link: PathBuf,
        #[arg(long)]
        no_link: bool,
        /// Lock key to plan for (default: this machine). Lets a dev host print
        /// the Linux plan with --dry-run.
        #[arg(long, value_name = "OS_ARCH")]
        platform: Option<String>,
    },
    /// Show the pinned release and what is installed.
    Status {
        #[arg(long, default_value = DEFAULT_DIR, value_name = "DIR")]
        dir: PathBuf,
        #[arg(long, default_value = DEFAULT_LINK, value_name = "PATH")]
        link: PathBuf,
    },
    /// Remove the installed release and the link that points into it.
    Remove {
        #[arg(long, default_value = DEFAULT_DIR, value_name = "DIR")]
        dir: PathBuf,
        #[arg(long, default_value = DEFAULT_LINK, value_name = "PATH")]
        link: PathBuf,
    },
}

/// Everything an install will do, computed before anything touches disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Plan {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub archive: PathBuf,
    pub extract_dir: PathBuf,
    pub binary: PathBuf,
    pub link: Option<PathBuf>,
}

pub(crate) fn lock() -> Result<Lock> {
    let lock: Lock =
        serde_json::from_str(LOCK).context("parsing the embedded Knossos release lock")?;
    if lock.schema != "cameo-knossos-release-lock/v1" {
        bail!("unsupported Knossos release lock schema {}", lock.schema);
    }
    for (key, asset) in &lock.assets {
        validate_asset(key, asset)?;
    }
    Ok(lock)
}

fn validate_asset(key: &str, asset: &Asset) -> Result<()> {
    if asset.sha256.len() != 64 || !asset.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("lock asset {key} has an invalid sha256");
    }
    if !asset.url.starts_with("https://github.com/") {
        bail!("lock asset {key} must be an https://github.com/ release URL");
    }
    let binary = Path::new(&asset.binary);
    if binary.is_absolute()
        || binary
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        bail!("lock asset {key} names an unsafe binary path");
    }
    Ok(())
}

pub(crate) fn platform_key() -> String {
    format!("{}_{}", std::env::consts::OS, std::env::consts::ARCH)
}

pub(crate) fn asset_for<'a>(lock: &'a Lock, key: &str) -> Result<&'a Asset> {
    lock.assets.get(key).ok_or_else(|| {
        anyhow!(
            "no Knossos {} release asset for this platform ({key}); available: {}",
            lock.version,
            lock.assets.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })
}

pub(crate) fn plan(lock: &Lock, key: &str, dir: &Path, link: Option<&Path>) -> Result<Plan> {
    let asset = asset_for(lock, key)?;
    let file_name = asset
        .url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow!("lock asset {key} URL has no file name"))?;
    let extract_dir = dir.join("versions").join(&lock.version);
    Ok(Plan {
        version: lock.version.clone(),
        url: asset.url.clone(),
        sha256: asset.sha256.clone(),
        size: asset.size,
        archive: dir.join("downloads").join(file_name),
        extract_dir: extract_dir.clone(),
        binary: extract_dir.join(&asset.binary),
        link: link.map(Path::to_path_buf),
    })
}

pub(crate) fn run(cli: &super::Cli, args: &Args) -> Result<()> {
    match &args.action {
        Action::Install {
            dir,
            link,
            no_link,
            platform,
        } => install(
            cli,
            dir,
            (!no_link).then_some(link.as_path()),
            platform.as_deref(),
        ),
        Action::Status { dir, link } => status(cli, dir, link),
        Action::Remove { dir, link } => remove(cli, dir, link),
    }
}

fn install(
    cli: &super::Cli,
    dir: &Path,
    link: Option<&Path>,
    platform: Option<&str>,
) -> Result<()> {
    let lock = lock()?;
    let key = platform.map(str::to_string).unwrap_or_else(platform_key);
    let plan = plan(&lock, &key, dir, link)?;
    if cli.dry_run {
        if cli.json {
            println!("{}", serde_json::to_string_pretty(&plan)?);
        } else {
            println!("Knossos {} would be installed:", plan.version);
            println!("  download {} ({} bytes)", plan.url, plan.size);
            println!("  verify   sha256 {}", plan.sha256);
            println!("  extract  {}", plan.extract_dir.display());
            println!("  binary   {}", plan.binary.display());
            match &plan.link {
                Some(link) => println!("  link     {} -> binary", link.display()),
                None => println!("  link     (skipped)"),
            }
        }
        return Ok(());
    }
    if !cfg!(unix) {
        bail!("cameo knossos install runs on the Cameo box (Linux); use --dry-run here.");
    }

    let downloads = plan
        .archive
        .parent()
        .ok_or_else(|| anyhow!("archive path has no parent"))?;
    std::fs::create_dir_all(downloads)
        .with_context(|| format!("creating {}", downloads.display()))?;

    if plan.archive.is_file() && cameo_models::file_sha256(&plan.archive)? == plan.sha256 {
        eprintln!("cameo: archive already present and verified");
    } else {
        let part = plan.archive.with_extension("zip.part");
        eprintln!(
            "cameo: downloading Knossos {}\n  from {}",
            plan.version, plan.url
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
            .arg(&plan.url)
            .status()
            .map_err(|e| anyhow!("could not run curl (is it installed?): {e}"))?;
        if !status.success() {
            bail!(
                "download failed (curl exit {:?}); partial file kept at {}",
                status.code(),
                part.display()
            );
        }
        let actual = cameo_models::file_sha256(&part)?;
        if actual != plan.sha256 {
            let _ = std::fs::remove_file(&part);
            bail!(
                "SHA-256 mismatch for the Knossos archive (got {actual}, want {}); refusing to install",
                plan.sha256
            );
        }
        std::fs::rename(&part, &plan.archive)
            .with_context(|| format!("finalising {}", plan.archive.display()))?;
        eprintln!("cameo: verified {}", plan.sha256);
    }

    if plan.extract_dir.exists() {
        std::fs::remove_dir_all(&plan.extract_dir)
            .with_context(|| format!("clearing {}", plan.extract_dir.display()))?;
    }
    std::fs::create_dir_all(&plan.extract_dir)
        .with_context(|| format!("creating {}", plan.extract_dir.display()))?;
    let status = Command::new("bsdtar")
        .arg("-xf")
        .arg(&plan.archive)
        .arg("-C")
        .arg(&plan.extract_dir)
        .status()
        .map_err(|e| anyhow!("could not run bsdtar (libarchive): {e}"))?;
    if !status.success() {
        bail!(
            "extracting {} failed (bsdtar exit {:?})",
            plan.archive.display(),
            status.code()
        );
    }
    if !plan.binary.is_file() {
        bail!(
            "archive did not contain the expected binary at {}",
            plan.binary.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&plan.binary, std::fs::Permissions::from_mode(0o755))?;
    }

    if let Some(link) = &plan.link {
        place_link(link, &plan.binary)?;
    }

    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "status": "installed",
                "version": plan.version,
                "binary": plan.binary,
                "link": plan.link,
            })
        );
    } else {
        println!(
            "Knossos {} installed at {}",
            plan.version,
            plan.binary.display()
        );
        if let Some(link) = &plan.link {
            println!("Linked as {}", link.display());
        }
        println!("Try: knossos --help");
    }
    Ok(())
}

/// Replace whatever is at `link` with a symlink to `target`. A bundled binary
/// from an older image is a regular file here and is replaced too.
#[cfg(unix)]
fn place_link(link: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    if std::fs::symlink_metadata(link).is_ok() {
        std::fs::remove_file(link).with_context(|| format!("replacing {}", link.display()))?;
    }
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("linking {} -> {}", link.display(), target.display()))
}

#[cfg(not(unix))]
fn place_link(_link: &Path, _target: &Path) -> Result<()> {
    bail!("linking is only supported on Linux")
}

fn installed_binary(dir: &Path, lock: &Lock, key: &str) -> Option<PathBuf> {
    plan(lock, key, dir, None)
        .ok()
        .map(|plan| plan.binary)
        .filter(|binary| binary.is_file())
}

fn status(cli: &super::Cli, dir: &Path, link: &Path) -> Result<()> {
    let lock = lock()?;
    let key = platform_key();
    let binary = installed_binary(dir, &lock, &key);
    let link_target = std::fs::read_link(link).ok();
    let reported = binary.as_ref().and_then(|binary| {
        Command::new(binary)
            .arg("--version")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    });
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "pinned": { "version": lock.version, "tag": lock.tag, "repository": lock.repository },
                "platform": key,
                "installed": binary,
                "link": link,
                "link_target": link_target,
                "reported_version": reported,
            }))?
        );
        return Ok(());
    }
    println!("Pinned:    Knossos {} ({})", lock.version, lock.tag);
    match &binary {
        Some(path) => println!("Installed: {}", path.display()),
        None => println!("Installed: no (run: cameo knossos install)"),
    }
    match &link_target {
        Some(target) => println!("Link:      {} -> {}", link.display(), target.display()),
        None => println!("Link:      {} (absent)", link.display()),
    }
    if let Some(version) = reported {
        println!("Reports:   {version}");
    }
    Ok(())
}

fn remove(cli: &super::Cli, dir: &Path, link: &Path) -> Result<()> {
    if cli.dry_run {
        println!(
            "Would remove {} and the link {}",
            dir.display(),
            link.display()
        );
        return Ok(());
    }
    let mut removed = Vec::new();
    if let Ok(target) = std::fs::read_link(link) {
        if target.starts_with(dir) {
            std::fs::remove_file(link).with_context(|| format!("removing {}", link.display()))?;
            removed.push(link.to_path_buf());
        }
    }
    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("removing {}", dir.display()))?;
        removed.push(dir.to_path_buf());
    }
    if cli.json {
        println!(
            "{}",
            serde_json::json!({ "status": "removed", "paths": removed })
        );
    } else if removed.is_empty() {
        println!("Nothing to remove.");
    } else {
        for path in removed {
            println!("Removed {}", path.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_lock_is_valid_and_pins_a_linux_asset() {
        let lock = lock().unwrap();
        assert_eq!(lock.schema, "cameo-knossos-release-lock/v1");
        let asset = asset_for(&lock, "linux_x86_64").unwrap();
        assert!(asset
            .url
            .starts_with("https://github.com/sadlowskik/Knossos-Harness/releases/download/"));
        assert_eq!(asset.sha256.len(), 64);
        assert!(asset.binary.ends_with("/knossos"));
    }

    #[test]
    fn the_plan_derives_every_path_from_the_lock_and_the_directory() {
        let lock = lock().unwrap();
        let dir = Path::new("/var/lib/cameo/knossos");
        let plan = plan(
            &lock,
            "linux_x86_64",
            dir,
            Some(Path::new("/usr/local/bin/knossos")),
        )
        .unwrap();
        assert_eq!(
            plan.archive,
            dir.join("downloads")
                .join("knossos-0.2.0-beta.2-linux-x86_64.zip")
        );
        assert_eq!(plan.extract_dir, dir.join("versions").join(&lock.version));
        assert!(plan.binary.starts_with(&plan.extract_dir));
        assert_eq!(
            plan.link.as_deref(),
            Some(Path::new("/usr/local/bin/knossos"))
        );
    }

    #[test]
    fn an_unknown_platform_is_refused_with_the_available_keys() {
        let lock = lock().unwrap();
        let error = asset_for(&lock, "plan9_mips").unwrap_err().to_string();
        assert!(
            error.contains("plan9_mips") && error.contains("linux_x86_64"),
            "{error}"
        );
    }

    #[test]
    fn lock_assets_must_be_github_https_with_a_safe_binary_path() {
        let bad_url = Asset {
            url: "http://example.com/x.zip".into(),
            sha256: "a".repeat(64),
            size: 1,
            binary: "x/knossos".into(),
        };
        assert!(validate_asset("k", &bad_url).is_err());
        let escaping = Asset {
            url: "https://github.com/o/r/releases/download/v1/x.zip".into(),
            sha256: "a".repeat(64),
            size: 1,
            binary: "../knossos".into(),
        };
        assert!(validate_asset("k", &escaping).is_err());
    }
}
