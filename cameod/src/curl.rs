//! Small, security-focused `curl` adapter for outbound JSON calls.
//!
//! Secrets and request bodies are supplied through curl's standard-input config,
//! never process arguments. On multi-user machines, command arguments are often
//! visible to other processes even when the Cameo state files are owner-only.

use std::io::Write;
use std::process::{Output, Stdio};

struct CurlPlan {
    args: [&'static str; 2],
    stdin_config: String,
}

fn quote_config(value: &str) -> Result<String, String> {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\u{000b}' => quoted.push_str("\\v"),
            character if character.is_control() => {
                return Err("curl request contains an unsupported control character".into())
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

fn plan_json_request(
    url: &str,
    method: &str,
    bearer: Option<&str>,
    body: Option<&[u8]>,
    timeout_secs: u64,
) -> Result<CurlPlan, String> {
    if !method.bytes().all(|byte| byte.is_ascii_uppercase()) || method.is_empty() {
        return Err("curl request method must contain only uppercase ASCII letters".into());
    }
    let mut config = String::from("silent\nshow-error\nfail\n");
    config.push_str(&format!("max-time = {timeout_secs}\n"));
    config.push_str("proto = \"=https\"\ntlsv1.2\n");
    config.push_str(&format!("request = {}\n", quote_config(method)?));
    config.push_str(&format!("url = {}\n", quote_config(url)?));
    if let Some(token) = bearer {
        config.push_str(&format!(
            "header = {}\n",
            quote_config(&format!("Authorization: Bearer {token}"))?
        ));
    }
    if let Some(bytes) = body {
        let body = std::str::from_utf8(bytes)
            .map_err(|_| "outbound JSON request body is not valid UTF-8".to_string())?;
        config.push_str("header = \"Content-Type: application/json\"\n");
        config.push_str(&format!("data-binary = {}\n", quote_config(body)?));
    }
    Ok(CurlPlan {
        args: ["--config", "-"],
        stdin_config: config,
    })
}

pub(crate) fn json_request(
    url: &str,
    method: &str,
    bearer: Option<&str>,
    body: Option<&[u8]>,
    timeout_secs: u64,
) -> Result<Output, String> {
    let plan = plan_json_request(url, method, bearer, body, timeout_secs)?;
    let mut child = std::process::Command::new("curl")
        .args(plan.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not run curl (is it installed?): {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "could not open curl standard input".to_string())?;
    stdin
        .write_all(plan.stdin_config.as_bytes())
        .map_err(|error| format!("could not configure curl request: {error}"))?;
    drop(stdin);
    child
        .wait_with_output()
        .map_err(|error| format!("could not wait for curl: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_and_body_never_enter_process_arguments() {
        let plan = plan_json_request(
            "https://hub.example/hub/pair",
            "POST",
            Some("device-secret"),
            Some(br#"{"pairing":"single-use-secret"}"#),
            10,
        )
        .unwrap();
        let args = plan.args.join(" ");
        assert_eq!(args, "--config -");
        assert!(!args.contains("device-secret"));
        assert!(!args.contains("single-use-secret"));
        assert!(plan.stdin_config.contains("device-secret"));
    }

    #[test]
    fn config_values_cannot_inject_new_options() {
        let plan = plan_json_request(
            "https://node.example/api",
            "POST",
            Some("secret\nurl = \"https://attacker.example\""),
            Some(b"{\"line\":\"one\\ntwo\"}"),
            10,
        )
        .unwrap();
        assert!(!plan.stdin_config.contains("secret\nurl"));
        assert!(plan.stdin_config.contains("secret\\nurl"));
        assert_eq!(
            plan.stdin_config
                .lines()
                .filter(|line| line.starts_with("url = "))
                .count(),
            1
        );
    }
}
