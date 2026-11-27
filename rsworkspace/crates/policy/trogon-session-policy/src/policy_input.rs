use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{Digest, IsolationTier};

use crate::{CapabilityInput, DestinationLocation, Limits, ModelPurpose, PrivacyMode, ReviewPolicy};

/// Administrative input. Never accept this document from the agent's workspace.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyInput {
    pub version: u32,
    pub privacy: PrivacyInput,
    pub destinations: Vec<DestinationInput>,
    pub model_routes: Vec<ModelRouteInput>,
    pub execution: ExecutionInput,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivacyInput {
    pub mode: PrivacyMode,
    pub allowed_destinations: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationInput {
    pub id: String,
    pub location: DestinationLocation,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRouteInput {
    pub purpose: ModelPurpose,
    pub destination: String,
    pub model: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionInput {
    pub network_allowlist: Vec<String>,
    pub limits: Limits,
    pub wasm_tools: Vec<WasmToolInput>,
    pub shell: Option<ShellInput>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WasmToolInput {
    pub id: String,
    pub component: Digest,
    pub capabilities: CapabilityInput,
    pub review: ReviewPolicy,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellInput {
    pub isolation: IsolationTier,
    pub review: ReviewPolicy,
}
