//! Offline release credits sourced from the consented, versioned tester roster.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

const INSTALLED_ROSTER: &str = "/usr/share/cameo/credits.json";
const EMBEDDED_ROSTER: &str = include_str!("../../testers/roster.json");
const MAX_ROSTER_BYTES: u64 = 1_048_576;

#[derive(Debug, Serialize, Deserialize)]
struct Roster {
    schema: String,
    consent: String,
    #[serde(default)]
    release_id: Option<String>,
    #[serde(default)]
    cutoff_at: Option<String>,
    testers: Vec<Tester>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Tester {
    name: String,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    machine: Option<String>,
    #[serde(default)]
    gpu: Option<String>,
    #[serde(default)]
    proved: Option<String>,
}

pub(crate) fn run(json: bool) -> Result<()> {
    let roster = load()?;
    validate(&roster)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&roster)?);
        return Ok(());
    }

    println!("Cameo release credits");
    if let Some(release) = roster.release_id.as_deref() {
        println!("Release: {release}");
    }
    if let Some(cutoff) = roster.cutoff_at.as_deref() {
        println!("Credit cutoff: {cutoff}");
    }
    if roster.testers.is_empty() {
        println!("No consented beta testers are listed yet.");
        return Ok(());
    }
    for tester in &roster.testers {
        let hardware = tester
            .machine
            .as_deref()
            .or(tester.gpu.as_deref())
            .unwrap_or("hardware report");
        println!("- {} — {hardware}", tester.name);
    }
    Ok(())
}

fn load() -> Result<Roster> {
    let path = Path::new(INSTALLED_ROSTER);
    let contents = match std::fs::metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.len() > MAX_ROSTER_BYTES {
                bail!("installed credits roster is not a bounded regular file");
            }
            std::fs::read_to_string(path).context("reading installed credits roster")?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => EMBEDDED_ROSTER.to_string(),
        Err(error) => return Err(error).context("inspecting installed credits roster"),
    };
    serde_json::from_str(&contents).context("parsing credits roster")
}

fn validate(roster: &Roster) -> Result<()> {
    if roster.schema != "cameo-testers/v1" {
        bail!("unsupported credits roster schema: {}", roster.schema);
    }
    if roster.testers.len() > 10_000 {
        bail!("credits roster exceeds 10,000 entries");
    }
    validate_text(&roster.consent, 256, "consent")?;
    for tester in &roster.testers {
        validate_text(&tester.name, 80, "tester name")?;
        for (value, max, label) in [
            (tester.date.as_deref(), 32, "date"),
            (tester.machine.as_deref(), 160, "machine"),
            (tester.gpu.as_deref(), 160, "GPU"),
            (tester.proved.as_deref(), 500, "evidence summary"),
        ] {
            if let Some(value) = value {
                validate_text(value, max, label)?;
            }
        }
    }
    Ok(())
}

fn validate_text(value: &str, max: usize, label: &str) -> Result<()> {
    if value.trim().is_empty()
        || value.chars().count() > max
        || value.chars().any(|character| character.is_control())
    {
        bail!("credits {label} is empty, oversized, or contains controls");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_roster_is_valid() {
        let roster: Roster = serde_json::from_str(EMBEDDED_ROSTER).unwrap();
        validate(&roster).unwrap();
    }

    #[test]
    fn roster_rejects_terminal_controls() {
        let roster = Roster {
            schema: "cameo-testers/v1".into(),
            consent: "consented".into(),
            release_id: None,
            cutoff_at: None,
            testers: vec![Tester {
                name: "bad\u{1b}[31m".into(),
                date: None,
                machine: None,
                gpu: None,
                proved: None,
            }],
        };
        assert!(validate(&roster).is_err());
    }
}
