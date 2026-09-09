//! The node-side hub agent: a node that phones home.
//!
//! When cameod is started with a hub URL, Cameo Link runs in a background thread.
//! It uses a persisted paired identity when available, redeems a one-time code on
//! first join, or takes the explicitly configured legacy token path. The node
//! reaches Cameo Mesh and heartbeats its live self-description; the hub does not
//! need inbound access merely to discover it.
//!
//! Network I/O shells out to `curl`, matching the CLI's
//! external-tool-not-a-linked-HTTP-stack stance. Everything that builds a request
//! body is pure and unit-tested; only [`post`] touches the network.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, info, warn};

/// How the node was told to find the hub and describe itself back to it.
#[derive(Clone)]
pub struct HubConfig {
    /// The hub's HTTPS base URL (no trailing slash needed).
    pub hub_url: String,
    /// Presented to the hub (`Authorization: Bearer …`) to authorize enrollment.
    pub farm_token: Option<String>,
    /// Single-use code for first enrollment. The resulting device credential is
    /// written to `credential_file` and the code is never persisted.
    pub pairing_code: Option<String>,
    pub credential_file: Option<PathBuf>,
    /// This node's advertised name.
    pub node_name: String,
    /// HTTPS base URL the hub should call back to reach this node's `/api`.
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

/// POST JSON to `<hub>/<path>` with an optional enrollment credential. Returns the response
/// body on success, or an error describing the failed curl. The one network seam.
fn post(cfg: &HubConfig, path: &str, body: &str, token: Option<&str>) -> Result<Vec<u8>, String> {
    let url = format!("{}/{}", base(&cfg.hub_url), path);
    let out = cameo_net_strategy::curl::json_request(
        &url,
        "POST",
        token,
        Some(body.as_bytes()),
        10,
        cameo_net_strategy::curl::HTTPS_ONLY,
    )?;
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
    let token = cfg
        .farm_token
        .as_deref()
        .ok_or_else(|| "legacy registration requires a farm token".to_string())?;
    let resp = post(cfg, "hub/register", &body, Some(token))?;
    node_id_from_response(&resp)
        .ok_or_else(|| "hub accepted the registration but returned no node_id".to_string())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DeviceCredential {
    node_id: String,
    device_credential: String,
}

fn pair(
    cfg: &HubConfig,
    code: &str,
    describe: &impl Fn() -> Option<Value>,
) -> Result<DeviceCredential, String> {
    let registration: Value = serde_json::from_str(&registration_body(cfg, describe().as_ref()))
        .map_err(|e| format!("building pairing registration: {e}"))?;
    let body = json!({ "code": code, "registration": registration }).to_string();
    let response = post(cfg, "hub/pair", &body, None)?;
    serde_json::from_slice(&response)
        .map_err(|e| format!("hub returned an invalid pairing credential: {e}"))
}

fn validate_device_credential(credential: &DeviceCredential) -> Result<(), String> {
    if credential.node_id.trim().is_empty() || credential.node_id.len() > 256 {
        return Err("stored device credential has an invalid node_id".into());
    }
    if credential.device_credential.len() != 64
        || !credential
            .device_credential
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("stored device credential has an invalid secret".into());
    }
    Ok(())
}

fn load_device_credential(path: &Path) -> Result<Option<DeviceCredential>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("inspecting {}: {e}", path.display())),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 8 * 1024 {
        return Err(format!(
            "device credential {} must be a regular file no larger than 8 KiB",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "device credential {} is accessible by group/others; run chmod 600",
                path.display()
            ));
        }
    }
    let bytes = std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let credential: DeviceCredential =
        serde_json::from_slice(&bytes).map_err(|e| format!("parsing {}: {e}", path.display()))?;
    validate_device_credential(&credential)?;
    Ok(Some(credential))
}

fn save_device_credential(path: &Path, credential: &DeviceCredential) -> Result<(), String> {
    validate_device_credential(credential)?;
    if path.exists() {
        return Err(format!(
            "refusing to overwrite existing device credential {}; remove it only when intentionally re-pairing",
            path.display()
        ));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    #[cfg(unix)]
    let created_parent = !parent.exists();
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("creating credential directory {}: {e}", parent.display()))?;
    #[cfg(unix)]
    if created_parent {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("securing credential directory {}: {e}", parent.display()))?;
    }
    let temp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("cameo-mesh-credential"),
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|e| format!("creating {}: {e}", temp.display()))?;
    let payload = serde_json::to_vec(credential)
        .map_err(|e| format!("serializing device credential: {e}"))?;
    if let Err(e) = file.write_all(&payload).and_then(|_| file.sync_all()) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("writing {}: {e}", temp.display()));
    }
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("committing device credential {}: {e}", path.display())
    })
}

fn run_paired(
    cfg: &HubConfig,
    credential: DeviceCredential,
    describe: &impl Fn() -> Option<Value>,
) {
    let interval = Duration::from_secs(HEARTBEAT_SECS);
    info!(hub = %base(&cfg.hub_url), node = %credential.node_id, "connected to paired hub");
    loop {
        let body = heartbeat_body(&credential.node_id, describe().as_ref());
        match post(
            cfg,
            "hub/heartbeat",
            &body,
            Some(&credential.device_credential),
        ) {
            Ok(response) => {
                let known = serde_json::from_slice::<Value>(&response)
                    .ok()
                    .and_then(|value| value.get("known").and_then(Value::as_bool))
                    .unwrap_or(false);
                if !known {
                    warn!("hub no longer recognizes this paired node; operator re-pairing is required");
                } else {
                    debug!(node = %credential.node_id, "paired heartbeat ok");
                }
            }
            Err(e) => warn!("paired heartbeat failed: {e}; retrying in {HEARTBEAT_SECS}s"),
        }
        std::thread::sleep(interval);
    }
}

/// The agent loop: register, then heartbeat forever, re-registering whenever the
/// hub reports it no longer knows this node (a `false` heartbeat, or any transport
/// failure). Runs until the process exits.
pub fn run(cfg: HubConfig, describe: impl Fn() -> Option<Value>) {
    if let Some(path) = cfg.credential_file.as_deref() {
        match load_device_credential(path) {
            Ok(Some(credential)) => return run_paired(&cfg, credential, &describe),
            Ok(None) => {}
            Err(e) => {
                warn!("cannot load paired credential: {e}");
                return;
            }
        }
    }
    if let Some(code) = cfg.pairing_code.as_deref() {
        let Some(path) = cfg.credential_file.as_deref() else {
            warn!("pairing code requires a credential file");
            return;
        };
        match pair(&cfg, code, &describe)
            .and_then(|credential| save_device_credential(path, &credential).map(|_| credential))
        {
            Ok(credential) => return run_paired(&cfg, credential, &describe),
            Err(e) => {
                warn!("device pairing failed: {e}");
                return;
            }
        }
    }
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
            match post(&cfg, "hub/heartbeat", &body, cfg.farm_token.as_deref()) {
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
            hub_url: "https://hub.lan:9090/".into(),
            farm_token: Some("farm-secret".into()),
            pairing_code: None,
            credential_file: None,
            node_name: "box-a".into(),
            callback_address: "https://node-a.example:9090".into(),
            node_key: Some("node-key".into()),
        }
    }

    #[test]
    fn registration_body_carries_identity_callback_and_description() {
        let desc = json!({ "gpus": [{ "gpu": { "model": "7900 XTX" } }] });
        let body: Value = serde_json::from_str(&registration_body(&cfg(), Some(&desc))).unwrap();
        assert_eq!(body["name"], "box-a");
        assert_eq!(body["address"], "https://node-a.example:9090");
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
        assert_eq!(base("https://hub.lan:9090/"), "https://hub.lan:9090");
        assert_eq!(base("https://hub.lan:9090"), "https://hub.lan:9090");
    }

    #[test]
    fn device_credential_is_atomically_saved_loaded_and_not_overwritten() {
        let suffix = crate::pairing::issue_device_credential().unwrap();
        let dir = std::env::temp_dir().join(format!("cameo-agent-test-{}", &suffix[..16]));
        let path = dir.join("mesh.json");
        let credential = DeviceCredential {
            node_id: "node-a".into(),
            device_credential: crate::pairing::issue_device_credential().unwrap(),
        };
        save_device_credential(&path, &credential).unwrap();
        let loaded = load_device_credential(&path).unwrap().unwrap();
        assert_eq!(loaded.node_id, credential.node_id);
        assert_eq!(loaded.device_credential, credential.device_credential);
        assert!(save_device_credential(&path, &credential).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
