//! Cameo's stable internal API contract.
//!
//! The CLI and (later) GUI are thin clients that speak this protocol to the core
//! service — neither ever touches a backend directly. The wire format is
//! JSON-RPC-style messages over a Unix domain socket (transport lands in Phase 2,
//! `docs/api.md`); this crate defines the **types**, which are the contract.
//!
//! Every message carries [`API_VERSION`] so client and server can detect a
//! mismatch and so the schema can evolve without silent breakage.

use serde::{Deserialize, Serialize};

/// Current API schema version. Bump on any breaking change to the types below.
pub const API_VERSION: u32 = 1;

/// Default Unix socket path for the core service.
pub const DEFAULT_SOCKET_PATH: &str = "/run/cameo/cameo.sock";

/// A request from a client to the core service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    /// Correlates responses to requests.
    pub id: u64,
    /// The method being invoked and its parameters.
    #[serde(flatten)]
    pub call: Call,
}

impl Request {
    pub fn new(id: u64, call: Call) -> Self {
        Self {
            version: API_VERSION,
            id,
            call,
        }
    }
}

/// The set of methods the core service exposes. Internally tagged by `method`,
/// so a request looks like `{"method":"model.run","params":{...}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Call {
    /// Report detected GPU(s), tier, and active backend.
    #[serde(rename = "gpu.status")]
    GpuStatus,
    /// Run inference on a model.
    #[serde(rename = "model.run")]
    ModelRun(ModelRunParams),
    /// Quantize a model to a target level.
    #[serde(rename = "model.quantize")]
    ModelQuantize(ModelQuantizeParams),
    /// Start a training run (Tier 1/2 only).
    #[serde(rename = "train.start")]
    TrainStart(TrainStartParams),
    /// Produce an install plan for the detected hardware.
    #[serde(rename = "install.plan")]
    InstallPlan,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRunParams {
    pub model: String,
    /// Optional explicit backend override (`"vulkan"` / `"rocm"`); `None` = auto.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelQuantizeParams {
    pub model: String,
    /// Quantization level, e.g. `"Q4_K_M"`, `"Q5_K_M"`, `"Q8_0"`.
    pub level: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrainStartParams {
    /// Path to the training config.
    pub config: String,
}

/// A response from the core service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub version: u32,
    pub id: u64,
    #[serde(flatten)]
    pub result: ApiResult,
}

impl Response {
    pub fn ok(id: u64, data: serde_json::Value) -> Self {
        Self {
            version: API_VERSION,
            id,
            result: ApiResult::Ok { data },
        }
    }

    pub fn error(id: u64, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            version: API_VERSION,
            id,
            result: ApiResult::Error {
                code: code.into(),
                message: message.into(),
            },
        }
    }
}

/// Success or failure payload, tagged by `status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ApiResult {
    Ok { data: serde_json::Value },
    Error { code: String, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrips_with_method_tag() {
        let req = Request::new(
            7,
            Call::ModelRun(ModelRunParams {
                model: "qwen".into(),
                backend: Some("vulkan".into()),
            }),
        );
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"method\":\"model.run\""));
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn unit_variant_serializes_without_params() {
        let req = Request::new(1, Call::GpuStatus);
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"method\":\"gpu.status\""));
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn response_ok_and_error_roundtrip() {
        let ok = Response::ok(1, serde_json::json!({"tier": 2}));
        let back: Response = serde_json::from_str(&serde_json::to_string(&ok).unwrap()).unwrap();
        assert_eq!(ok, back);

        let err = Response::error(2, "tier_unsupported", "training needs Tier 1/2");
        let back: Response = serde_json::from_str(&serde_json::to_string(&err).unwrap()).unwrap();
        assert_eq!(err, back);
    }
}
