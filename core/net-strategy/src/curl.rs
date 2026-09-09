//! Hardened external `curl` adapter shared by Cameo clients.
//!
//! Secrets and JSON bodies are supplied through curl's standard-input config,
//! never process arguments. Protocol and response-size policy are fixed by the
//! caller's trust boundary rather than being accepted as arbitrary flags.

use std::io::Write;
use std::process::{Output, Stdio};

const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy)]
pub struct Policy {
    protocols: &'static str,
}

pub const HTTPS_ONLY: Policy = Policy {
    protocols: "=https",
};
pub const HTTP_OR_HTTPS: Policy = Policy {
    protocols: "=http,https",
};

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
    policy: Policy,
) -> Result<CurlPlan, String> {
    if !method.bytes().all(|byte| byte.is_ascii_uppercase()) || method.is_empty() {
        return Err("curl request method must contain only uppercase ASCII letters".into());
    }
    if timeout_secs == 0 || timeout_secs > 300 {
        return Err("curl timeout must be between 1 and 300 seconds".into());
    }
    let mut config = String::from("silent\nshow-error\nfail\n");
    config.push_str(&format!("connect-timeout = {}\n", timeout_secs.min(5)));
    config.push_str(&format!("max-time = {timeout_secs}\n"));
    config.push_str(&format!("max-filesize = {MAX_RESPONSE_BYTES}\n"));
    config.push_str(&format!("proto = \"{}\"\n", policy.protocols));
    if policy.protocols == "=https" {
        config.push_str("tlsv1.2\n");
    }
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

pub fn json_request(
    url: &str,
    method: &str,
    bearer: Option<&str>,
    body: Option<&[u8]>,
    timeout_secs: u64,
    policy: Policy,
) -> Result<Output, String> {
    let plan = plan_json_request(url, method, bearer, body, timeout_secs, policy)?;
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
            HTTPS_ONLY,
        )
        .unwrap();
        let args = plan.args.join(" ");
        assert_eq!(args, "--config -");
        assert!(!args.contains("device-secret"));
        assert!(!args.contains("single-use-secret"));
        assert!(plan.stdin_config.contains("device-secret"));
        assert!(plan.stdin_config.contains("proto = \"=https\""));
    }

    #[test]
    fn config_values_cannot_inject_new_options() {
        let plan = plan_json_request(
            "http://node.example/api",
            "POST",
            Some("secret\nurl = \"https://attacker.example\""),
            Some(b"{\"line\":\"one\\ntwo\"}"),
            10,
            HTTP_OR_HTTPS,
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

    #[test]
    fn request_policy_is_bounded() {
        assert!(
            plan_json_request("https://example.test", "GET", None, None, 0, HTTPS_ONLY).is_err()
        );
        let plan = plan_json_request(
            "http://node.example/api",
            "GET",
            None,
            None,
            30,
            HTTP_OR_HTTPS,
        )
        .unwrap();
        assert!(plan.stdin_config.contains("max-filesize = 1048576"));
        assert!(plan.stdin_config.contains("proto = \"=http,https\""));
    }
}
