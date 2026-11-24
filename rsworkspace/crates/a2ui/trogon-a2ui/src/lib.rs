#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! Manually maintained A2UI v1.0 JSON protocol types.
//!
//! A2UI catalogs remain application-defined. Component properties and action
//! context therefore stay as JSON values while the stable protocol envelopes
//! are represented by Rust structs.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The A2UI protocol version supported by this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolVersion {
    V1,
}

impl ProtocolVersion {
    pub const V1_0: Self = Self::V1;

    pub const fn as_str(self) -> &'static str {
        "1.0"
    }
}

impl Serialize for ProtocolVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProtocolVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value == "1.0" {
            Ok(Self::V1)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported A2UI protocol version {value:?}"
            )))
        }
    }
}

/// A catalog-defined component instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub id: String,
    pub component: String,
    #[serde(flatten)]
    pub properties: Map<String, Value>,
}

impl Component {
    pub fn new(id: impl Into<String>, component: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            component: component.into(),
            properties: Map::new(),
        }
    }

    pub fn with_property(mut self, name: impl Into<String>, value: Value) -> Self {
        self.properties.insert(name.into(), value);
        self
    }
}

/// A renderer-side UI surface's component tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSurface {
    pub surface_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_data_model: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<Component>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_model: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SurfaceMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateComponents {
    pub surface_id: String,
    pub components: Vec<Component>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDataModel {
    pub surface_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSurface {
    pub surface_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallRendererFunction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
    pub function_call_id: String,
    pub call_function: RendererFunctionCall,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionCall {
    #[serde(rename = "@call")]
    pub call: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RendererFunctionCall {
    #[serde(rename = "@call")]
    pub call: String,
    pub catalog_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FunctionResponse {
    Value {
        #[serde(rename = "functionCallId")]
        function_call_id: String,
        value: Value,
    },
    Error {
        #[serde(rename = "functionCallId")]
        function_call_id: String,
        error: FunctionError,
    },
}

pub type AgentFunctionResponse = FunctionResponse;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message: Option<String>,
    pub surface_id: String,
    pub source_component_id: String,
    pub timestamp: String,
    pub context: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SurfaceMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallAgentFunction {
    pub surface_id: String,
    pub function_call_id: String,
    pub call_function: FunctionCall,
}

pub type RendererFunctionResponse = FunctionResponse;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RendererError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentToRendererMessage {
    pub version: ProtocolVersion,
    #[serde(flatten)]
    pub message: AgentToRendererPayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentToRendererPayload {
    CreateSurface {
        #[serde(rename = "createSurface")]
        create_surface: CreateSurface,
    },
    UpdateComponents {
        #[serde(rename = "updateComponents")]
        update_components: UpdateComponents,
    },
    UpdateDataModel {
        #[serde(rename = "updateDataModel")]
        update_data_model: UpdateDataModel,
    },
    DeleteSurface {
        #[serde(rename = "deleteSurface")]
        delete_surface: DeleteSurface,
    },
    CallRendererFunction {
        #[serde(rename = "callRendererFunction")]
        call_renderer_function: CallRendererFunction,
    },
    AgentFunctionResponse {
        #[serde(rename = "agentFunctionResponse")]
        agent_function_response: AgentFunctionResponse,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RendererToAgentMessage {
    pub version: ProtocolVersion,
    #[serde(flatten)]
    pub message: RendererToAgentPayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RendererToAgentPayload {
    Action {
        action: Action,
    },
    CallAgentFunction {
        #[serde(rename = "callAgentFunction")]
        call_agent_function: CallAgentFunction,
    },
    RendererFunctionResponse {
        #[serde(rename = "rendererFunctionResponse")]
        renderer_function_response: RendererFunctionResponse,
    },
    Error {
        error: RendererError,
    },
}

#[cfg(test)]
mod tests;
