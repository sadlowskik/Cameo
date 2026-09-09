//! Local-first, explicit-consent beta hardware reports.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use clap::ValueEnum;
use serde::Serialize;
use sha2::{Digest, Sha256};

const DEFAULT_ENDPOINT: &str = "https://cameoconstruct.xyz/api/hardware-reports";
const MANUAL_URL: &str = "https://cameoconstruct.xyz/testers.html";
const MAX_NOTE_CHARS: usize = 1_000;
const MAX_ERROR_CHARS: usize = 2_000;

#[derive(clap::Args)]
pub(crate) struct Args {
    /// New local report file. Defaults to cameo-report-<id>.json.
    #[arg(long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Submit after local creation, exact preview, and interactive confirmation.
    #[arg(long)]
    submit: bool,

    /// Submission endpoint. HTTPS is mandatory.
    #[arg(long, default_value = DEFAULT_ENDPOINT, value_name = "HTTPS_URL")]
    endpoint: String,

    /// Cameo/ISO boot outcome. Omitted means observed pass only on a Cameo image.
    #[arg(long, value_enum)]
    boot: Option<Outcome>,

    /// Result of a real model load and generation. Defaults to not-tested.
    #[arg(long, value_enum, default_value_t = Outcome::NotTested)]
    inference: Outcome,

    /// Optional tester note. This is included verbatim after control checks.
    #[arg(long)]
    note: Option<String>,

    /// Optional redacted error excerpt; raw journals are never collected.
    #[arg(long)]
    error_excerpt: Option<String>,

    /// Request public credit in the site, repository, and immutable release artifacts.
    #[arg(long)]
    credit: bool,

    /// Display name/handle for permanent public credit; never required for a report.
    #[arg(long, value_name = "HANDLE")]
    credit_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Passed,
    Failed,
    Degraded,
    NotTested,
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Degraded => "degraded",
            Self::NotTested => "not-tested",
        })
    }
}

#[derive(Serialize)]
struct HardwareReport {
    schema_version: &'static str,
    report_id: String,
    generated_at_unix: u64,
    artifact: super::system_facts::ArtifactFacts,
    platform: super::system_facts::PlatformFacts,
    hardware: super::system_facts::HardwareFacts,
    outcomes: Outcomes,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_excerpt: Option<String>,
    privacy: Privacy,
}

#[derive(Serialize)]
struct Outcomes {
    boot: OutcomeFact,
    gpu_detection: OutcomeFact,
    inference: OutcomeFact,
}

#[derive(Serialize)]
struct OutcomeFact {
    result: Outcome,
    evidence_source: &'static str,
}

#[derive(Serialize)]
struct Privacy {
    allowlisted_fields_only: bool,
    hostname: bool,
    username: bool,
    ip_or_mac: bool,
    serial_numbers: bool,
    credentials: bool,
    raw_logs: bool,
    prompts_or_model_output: bool,
}

#[derive(Serialize)]
struct Submission<'a> {
    schema_version: &'static str,
    report: &'a HardwareReport,
    credit_request: CreditRequest,
}

#[derive(Serialize)]
struct CreditRequest {
    publish: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    permanence_acknowledged: bool,
}

pub(crate) fn run(cli: &super::Cli, args: &Args) -> Result<()> {
    let note = clean_user_text(args.note.as_deref(), MAX_NOTE_CHARS, "note")?;
    let error_excerpt = clean_user_text(
        args.error_excerpt.as_deref(),
        MAX_ERROR_CHARS,
        "error excerpt",
    )?;
    let credit_name = clean_user_text(args.credit_name.as_deref(), 80, "credit name")?;
    validate_credit(
        args.credit,
        credit_name.as_deref(),
        args.credit_name.is_some(),
    )?;

    let facts = super::system_facts::collect(cli);
    let generated_at_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs();
    let on_cameo_image = facts.artifact.iso_build_id.is_some();
    let boot = args.boot.unwrap_or(if on_cameo_image {
        Outcome::Passed
    } else {
        Outcome::NotTested
    });
    let boot_source = if args.boot.is_some() {
        "tester_asserted"
    } else if on_cameo_image {
        "observed"
    } else {
        "unavailable"
    };
    let detection = if facts.hardware.status == "observed"
        && facts.hardware.gpus.iter().any(|gpu| gpu.vendor == "AMD")
    {
        Outcome::Passed
    } else {
        Outcome::Failed
    };
    let detection_source = if facts.hardware.status == "observed" {
        "observed"
    } else {
        "unavailable"
    };
    let report = HardwareReport {
        schema_version: "cameo-hardware-report/v1",
        report_id: report_id()?,
        generated_at_unix,
        artifact: facts.artifact,
        platform: facts.platform,
        hardware: facts.hardware,
        outcomes: Outcomes {
            boot: OutcomeFact {
                result: boot,
                evidence_source: boot_source,
            },
            gpu_detection: OutcomeFact {
                result: detection,
                evidence_source: detection_source,
            },
            inference: OutcomeFact {
                result: args.inference,
                evidence_source: if args.inference == Outcome::NotTested {
                    "unavailable"
                } else {
                    "tester_asserted"
                },
            },
        },
        note,
        error_excerpt,
        privacy: Privacy {
            allowlisted_fields_only: true,
            hostname: false,
            username: false,
            ip_or_mac: false,
            serial_numbers: false,
            credentials: false,
            raw_logs: false,
            prompts_or_model_output: false,
        },
    };

    let mut local_bytes = serde_json::to_vec_pretty(&report)?;
    local_bytes.push(b'\n');
    let digest = sha256_hex(&local_bytes);
    let path = args
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("cameo-report-{}.json", report.report_id)));

    println!("{}", String::from_utf8_lossy(&local_bytes));
    write_new(&path, &local_bytes)?;
    eprintln!("Report saved to {}", path.display());
    eprintln!("SHA-256: {digest}");

    if !args.submit {
        return Ok(());
    }

    validate_endpoint(&args.endpoint)?;
    let submission = Submission {
        schema_version: "cameo-hardware-submission/v1",
        report: &report,
        credit_request: CreditRequest {
            publish: args.credit,
            display_name: credit_name,
            permanence_acknowledged: args.credit,
        },
    };
    let payload = serde_json::to_vec_pretty(&submission)?;
    let submission_digest = sha256_hex(&payload);
    println!("Submission destination: {}", args.endpoint);
    println!(
        "Exact submission payload:\n{}",
        String::from_utf8_lossy(&payload)
    );
    eprintln!("Submission SHA-256: {submission_digest}");
    confirm(&submission_digest)?;

    match submit(&args.endpoint, &payload) {
        Ok(receipt) => {
            eprintln!("Submission accepted for moderation: {receipt}");
            Ok(())
        }
        Err(error) => {
            eprintln!("Submission failed: {error}");
            eprintln!("Your local report remains at {}", path.display());
            eprintln!("Submit it manually at {MANUAL_URL}");
            eprintln!(
                "Paste-ready payload:\n{}",
                String::from_utf8_lossy(&payload)
            );
            Err(anyhow!("hardware report submission failed"))
        }
    }
}

fn validate_credit(credit: bool, clean_name: Option<&str>, name_was_supplied: bool) -> Result<()> {
    if credit && clean_name.is_none() {
        bail!("--credit requires --credit-name; omit both to submit anonymously");
    }
    if !credit && name_was_supplied {
        bail!("--credit-name requires the separate --credit consent flag");
    }
    Ok(())
}

fn clean_user_text(value: Option<&str>, max: usize, label: &str) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > max {
        bail!("{label} exceeds {max} characters");
    }
    if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        bail!("{label} contains unsupported control characters");
    }
    if value.contains('\u{1b}') {
        bail!("{label} contains a terminal escape sequence");
    }
    Ok(Some(value.to_string()))
}

fn report_id() -> Result<String> {
    let mut random = [0_u8; 12];
    getrandom::fill(&mut random).map_err(|error| anyhow!("generating report ID: {error}"))?;
    Ok(format!("chr_{}", hex(&random)))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex(&digest)
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| {
        format!(
            "creating {} (existing files are never overwritten)",
            path.display()
        )
    })?;
    if !file.metadata()?.is_file() {
        bail!("report output must be a regular file");
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn confirm(digest: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("submission requires an interactive terminal; the local report was still saved");
    }
    let token = &digest[..12];
    eprint!("Type SUBMIT {token} to send this exact payload: ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if answer.trim() != format!("SUBMIT {token}") {
        bail!("submission cancelled; the local report was still saved");
    }
    Ok(())
}

fn validate_endpoint(endpoint: &str) -> Result<()> {
    let Some(rest) = endpoint.strip_prefix("https://") else {
        bail!("report submission endpoint must use https://");
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty()
        || authority.contains('@')
        || endpoint.contains('\\')
        || endpoint.contains('#')
        || endpoint.chars().any(char::is_whitespace)
    {
        bail!("report submission endpoint is malformed");
    }
    Ok(())
}

fn quote_curl_config(value: &str) -> Result<String> {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            character if character.is_control() => {
                bail!("submission contains an unsupported control character")
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

fn submit(endpoint: &str, payload: &[u8]) -> Result<String> {
    let payload = std::str::from_utf8(payload).context("submission is not UTF-8")?;
    let config = format!(
        "silent\nshow-error\nfail\nconnect-timeout = 5\nmax-time = 15\nmax-filesize = 65536\nproto = \"=https\"\ntlsv1.2\nrequest = \"POST\"\nheader = \"Content-Type: application/json\"\nurl = {}\ndata-binary = {}\n",
        quote_curl_config(endpoint)?,
        quote_curl_config(payload)?
    );
    let mut child = Command::new("curl")
        .args(["--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not run curl (is it installed?)")?;
    child
        .stdin
        .take()
        .context("could not open curl stdin")?
        .write_all(config.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        stderr.truncate(512);
        bail!("curl exited {:?}: {}", output.status.code(), stderr.trim());
    }
    let receipt_bytes = &output.stdout[..output.stdout.len().min(65_536)];
    Ok(String::from_utf8_lossy(receipt_bytes).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_is_https_without_userinfo_or_fragments() {
        assert!(validate_endpoint(DEFAULT_ENDPOINT).is_ok());
        assert!(validate_endpoint("http://example.test/report").is_err());
        assert!(validate_endpoint("https://user@example.test/report").is_err());
        assert!(validate_endpoint("https://example.test/report#secret").is_err());
    }

    #[test]
    fn user_text_is_bounded_and_rejects_terminal_escapes() {
        assert_eq!(
            clean_user_text(Some(" hello "), 8, "note")
                .unwrap()
                .as_deref(),
            Some("hello")
        );
        assert!(clean_user_text(Some("012345678"), 8, "note").is_err());
        assert!(clean_user_text(Some("hello\u{1b}[31m"), 80, "note").is_err());
    }

    #[test]
    fn curl_values_cannot_inject_configuration() {
        let quoted = quote_curl_config("ok\nurl = \"https://attacker.test\"").unwrap();
        assert!(!quoted.contains("ok\nurl"));
        assert!(quoted.contains("ok\\nurl"));
    }

    #[test]
    fn write_never_overwrites() {
        let path = std::env::temp_dir().join(format!("cameo-report-test-{}", report_id().unwrap()));
        write_new(&path, b"first").unwrap();
        assert!(write_new(&path, b"second").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        std::fs::remove_file(path).unwrap();
    }
}
