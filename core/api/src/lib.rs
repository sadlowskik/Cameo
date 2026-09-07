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
use std::sync::OnceLock;

/// Current API schema version. Bump on any breaking change to the types below.
pub const API_VERSION: u32 = 1;

/// Default Unix socket path for the core service.
pub const DEFAULT_SOCKET_PATH: &str = "/run/cameo/cameo.sock";

/// Versioned product-capability manifest shared by the daemon, harness, UI, and
/// release checks. The checked-in JSON is the source of truth; this typed view
/// prevents a typo or missing field from silently becoming a product promise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityManifest {
    pub contract_version: String,
    pub protocol_major: u16,
    pub inference: InferenceCapabilities,
    pub harness: HarnessCapabilities,
    pub models: ModelCapabilities,
    pub mesh: MeshCapabilities,
    pub security: SecurityCapabilities,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    pub maturity: CapabilityMaturity,
    pub available: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityMaturity {
    Stable,
    Preview,
    Planned,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InferenceCapabilities {
    pub openai_compatible_gateway: Capability,
    pub streaming: Capability,
    pub native_tool_calls: Capability,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessCapabilities {
    pub knossos_session_control: Capability,
    pub vram_leases: Capability,
    pub context_discovery: Capability,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub verified_downloads: Capability,
    pub hardware_recommendation: Capability,
    pub one_action_setup: Capability,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshCapabilities {
    pub scheduling: Capability,
    pub enrollment_transport: String,
    pub device_pairing: Capability,
    pub mutual_tls: Capability,
    pub distributed_model_sharding: Capability,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecurityCapabilities {
    pub role_scoped_keys: Capability,
    pub local_harness_bypass: Capability,
    pub request_rate_limits: Capability,
    pub release_security_review: Capability,
}

/// Parse the embedded, version-controlled manifest once. A malformed release
/// artifact is a programmer error and is caught by unit tests before packaging.
pub fn capability_manifest() -> &'static CapabilityManifest {
    static MANIFEST: OnceLock<CapabilityManifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../contracts/cameo-capabilities-v1.json"
        ))
        .expect("checked-in Cameo capability manifest must match its typed contract")
    })
}

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

    #[test]
    fn checked_in_capability_manifest_is_typed_and_truthful() {
        let manifest = capability_manifest();
        assert_eq!(manifest.contract_version, "cameo-capabilities/v1");
        assert_eq!(manifest.protocol_major, 1);
        assert!(manifest.inference.openai_compatible_gateway.available);
        assert!(manifest.mesh.device_pairing.available);
        assert!(!manifest.mesh.mutual_tls.available);
        assert_eq!(
            manifest.mesh.enrollment_transport,
            "paired_bearer_or_legacy_farm_token"
        );
        assert!(manifest.models.hardware_recommendation.available);
        assert!(manifest.models.one_action_setup.available);
        assert!(manifest.security.request_rate_limits.available);
    }

    #[test]
    fn public_surfaces_use_mesh_names_and_preserve_non_goals() {
        let readme = include_str!("../../../README.md");
        let site = include_str!("../../../site/index.html");
        let mesh_guide = include_str!("../../../docs/cameo-mesh.md");
        for (name, surface) in [
            ("README", readme),
            ("website", site),
            ("Mesh guide", mesh_guide),
        ] {
            assert!(surface.contains("Cameo Mesh"), "{name} lost the Mesh name");
            assert!(surface.contains("Cameo Link"), "{name} lost the Link name");
        }
        let readme_words = readme.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(readme_words.contains("does not pool VRAM"));
        assert!(site.contains("does not combine VRAM"));
        assert!(site.contains("mutual TLS is not available yet"));

        let manifest = capability_manifest();
        assert!(!manifest.mesh.mutual_tls.available);
        assert!(!manifest.mesh.distributed_model_sharding.available);
    }
}
