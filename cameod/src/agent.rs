//! The node-side hub agent: a node that phones home.
//!
//! When cameod is started with a hub URL and a farm token (see [`crate::main`]),
//! this agent runs in a background thread: it POSTs the node's `/api/node`
//! self-description to `POST /hub/register`, then heartbeats on an interval so the
//! hub's roster stays live. It is the *outbound* half of the HiveOS inversion —
//! the node reaches the hub, so the hub never needs inbound access to the node to
//! learn it exists (only, later, to push work to it).
//!
//! Network I/O shells out to `curl`, matching the CLI's
//! external-tool-not-a-linked-HTTP-stack stance. Everything that builds a request
//! body is pure and unit-tested; only [`post`] touches the network.

use std::time::Duration;

use serde_json::{json, Value};
use tracing::{debug, info, warn};

/// How the node was told to find the hub and describe itself back to it.
#[derive(Clone)]
pub struct HubConfig {
    /// The hub's base URL, e.g. `http://hub.lan:9090` (no trailing slash needed).
    pub hub_url: String,
    /// Presented to the hub (`Authorization: Bearer …`) to authorize enrollment.
    pub farm_token: String,
    /// This node's advertised name.
    pub node_name: String,
    /// `host:port` the hub should call back to reach this node's `/api`.
    pub callback_address: String,
    /// This node's own console key, handed to the hub so it can push work to this
    /// node's authenticated `/api`. `None` when the node runs keyless.
    pub node_key: Option<String>,
}

/// Seconds between heartbeats. Comfortably under the hub's online window so a
/// healthy node never flaps offline on a single dropped beat.
const HEARTBEAT_SECS: u64 = 15;

/// The `POST /hub/register` body: identity, callback, and the current
/// self-description. Pure so its shape is unit-tested without a hub.
fn registration_body(cfg: &HubConfig, description: Option<&Value>) -> String {
    json!({
        "name": cfg.node_name,
        "address": cfg.callback_address,
        "key": cfg.node_key,
        "cameo_version": env!("CARGO_PKG_VERSION"),
        "node": description.cloned().unwrap_or(Value::Null),
    })
    .to_string()
}

/// The `POST /hub/heartbeat` body: which node, plus a refreshed description.
fn heartbeat_body(node_id: &str, description: Option<&Value>) -> String {
    json!({
        "node_id": node_id,
        "node": description.cloned().unwrap_or(Value::Null),
    })
    .to_string()
}

/// Trim a trailing slash so `join`ing a path never doubles it.
fn base(url: &str) -> &str {
    url.trim_end_matches('/')
}

/// POST a JSON body to `<hub>/<path>` with the farm token. Returns the response
/// body on success, or an error describing the failed curl. The one network seam.
fn post(cfg: &HubConfig, path: &str, body: &str) -> Result<Vec<u8>, String> {
    let url = format!("{}/{}", base(&cfg.hub_url), path);
    let out = std::process::Command::new("curl")
        .args(["-s", "--fail", "--max-time", "10", "-X", "POST"])
        .arg("-H")
        .arg(format!("Authorization: Bearer {}", cfg.farm_token))
        .args(["-H", "Content-Type: application/json", "-d", body, &url])
        .output()
        .map_err(|e| format!("could not run curl (is it installed?): {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "POST {url} failed (curl exit {:?})",
            out.status.code()
        ));
    }
    Ok(out.stdout)
}

/// Pull the assigned `node_id` out of a `POST /hub/register` response.
fn node_id_from_response(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()?
        .get("node_id")?
        .as_str()
        .map(str::to_string)
}

/// Register once, returning the hub-assigned `node_id`.
fn register(cfg: &HubConfig, describe: &impl Fn() -> Option<Value>) -> Result<String, String> {
    let body = registration_body(cfg, describe().as_ref());
    let resp = post(cfg, "hub/register", &body)?;
    node_id_from_response(&resp)
        .ok_or_else(|| "hub accepted the registration but returned no node_id".to_string())
}

/// The agent loop: register, then heartbeat forever, re-registering whenever the
/// hub reports it no longer knows this node (a `false` heartbeat, or any transport
/// failure). Runs until the process exits.
pub fn run(cfg: HubConfig, describe: impl Fn() -> Option<Value>) {
    let interval = Duration::from_secs(HEARTBEAT_SECS);
    loop {
        let node_id = match register(&cfg, &describe) {
            Ok(id) => {
                info!(hub = %base(&cfg.hub_url), node = %id, "registered with hub");
                id
            }
            Err(e) => {
                warn!("hub registration failed: {e}; retrying in {HEARTBEAT_SECS}s");
                std::thread::sleep(interval);
                continue;
            }
        };

        // Heartbeat until the hub forgets us or the transport breaks, then loop
        // back to a fresh registration.
        loop {
            std::thread::sleep(interval);
            let body = heartbeat_body(&node_id, describe().as_ref());
            match post(&cfg, "hub/heartbeat", &body) {
                Ok(resp) => {
                    let known = serde_json::from_slice::<Value>(&resp)
                        .ok()
                        .and_then(|v| v.get("known").and_then(Value::as_bool))
                        .unwrap_or(true);
                    if !known {
                        warn!("hub no longer knows this node; re-registering");
                        break;
                    }
                    debug!(node = %node_id, "heartbeat ok");
                }
                Err(e) => {
                    warn!("heartbeat failed: {e}; re-registering");
                    break;
                }
            }
        }
    }
}

/// Spawn [`run`] on a background thread. `describe` yields this node's current
/// `/api/node` body (or `None` when detection is unavailable, e.g. a non-Linux
/// dev host), and is called afresh on every beat so the hub sees live endpoints.
pub fn spawn(cfg: HubConfig, describe: impl Fn() -> Option<Value> + Send + 'static) {
    std::thread::spawn(move || run(cfg, describe));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> HubConfig {
        HubConfig {
            hub_url: "http://hub.lan:9090/".into(),
            farm_token: "farm-secret".into(),
            node_name: "box-a".into(),
            callback_address: "10.0.0.2:9090".into(),
            node_key: Some("node-key".into()),
        }
    }

    #[test]
    fn registration_body_carries_identity_callback_and_description() {
        let desc = json!({ "gpus": [{ "gpu": { "model": "7900 XTX" } }] });
        let body: Value = serde_json::from_str(&registration_body(&cfg(), Some(&desc))).unwrap();
        assert_eq!(body["name"], "box-a");
        assert_eq!(body["address"], "10.0.0.2:9090");
        assert_eq!(body["key"], "node-key");
        assert_eq!(body["node"]["gpus"][0]["gpu"]["model"], "7900 XTX");
        assert!(body["cameo_version"].is_string());
    }

    #[test]
    fn registration_body_without_a_description_sends_null_not_missing() {
        let body: Value = serde_json::from_str(&registration_body(&cfg(), None)).unwrap();
        assert_eq!(
            body["node"],
            Value::Null,
            "a dev host with no detection still enrolls"
        );
    }

    #[test]
    fn heartbeat_body_names_the_node_and_refreshes_the_description() {
        let desc = json!({ "endpoints": [{ "id": "x" }] });
        let body: Value = serde_json::from_str(&heartbeat_body("box-a", Some(&desc))).unwrap();
        assert_eq!(body["node_id"], "box-a");
        assert_eq!(body["node"]["endpoints"][0]["id"], "x");
    }

    #[test]
    fn node_id_is_read_from_the_register_response() {
        assert_eq!(
            node_id_from_response(br#"{"node_id":"box-a","known":true}"#).as_deref(),
            Some("box-a")
        );
        assert_eq!(node_id_from_response(br#"{}"#), None);
        assert_eq!(node_id_from_response(b"not json"), None);
    }

    #[test]
    fn base_strips_only_trailing_slashes() {
        assert_eq!(base("http://hub.lan:9090/"), "http://hub.lan:9090");
        assert_eq!(base("http://hub.lan:9090"), "http://hub.lan:9090");
    }
}
