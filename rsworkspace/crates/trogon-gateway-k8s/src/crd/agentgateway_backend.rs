//! `AgentgatewayBackend` CRD — Solo.io-shaped MCP backend declaration.
//!
//! Describes one MCP server (or family of related targets) and the
//! JWT validation rules that gate traffic to it. The
//! `resourceMetadata` map is forwarded into the gateway-hosted
//! `/.well-known/oauth-protected-resource/mcp/{backend}` document so
//! clients can discover the right authorization server. The
//! `agentgateway.dev/issuer-proxy` annotation is the well-known key
//! that points consent-flow clients at the gateway's OAuth issuer.

use std::collections::BTreeMap;

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const ISSUER_PROXY_ANNOTATION: &str = "agentgateway.dev/issuer-proxy";

#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "enterpriseagentgateway.solo.io",
    version = "v1alpha1",
    kind = "AgentgatewayBackend",
    plural = "agentgatewaybackends",
    shortname = "agwb",
    namespaced,
    status = "AgentgatewayBackendStatus",
    printcolumn = r#"{"name":"Targets","type":"integer","jsonPath":".spec.targets[*]"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct AgentgatewayBackendSpec {
    /// One or more upstream MCP endpoints. The gateway round-robins
    /// across targets when multiple are listed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<BackendTarget>,
    /// JWT validation applied to inbound requests before token
    /// exchange or elicitation runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwt_validation: Option<JwtValidation>,
    /// Free-form annotations surfaced in the per-backend OAuth
    /// Protected Resource Metadata document. The
    /// `agentgateway.dev/issuer-proxy` key is honored as the
    /// authorization server URL.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub resource_metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackendTarget {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JwtValidation {
    pub jwks_uri: String,
    pub issuer: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audiences: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentgatewayBackendStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<BackendCondition>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackendCondition {
    #[serde(rename = "type")]
    pub condition_type: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_transition_time: Option<String>,
}

impl AgentgatewayBackendSpec {
    /// Returns the configured issuer-proxy URL if the
    /// `agentgateway.dev/issuer-proxy` annotation is present.
    pub fn issuer_proxy(&self) -> Option<&str> {
        self.resource_metadata.get(ISSUER_PROXY_ANNOTATION).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_backend_with_jwt_validation_and_issuer_proxy() {
        let json = r#"{
            "targets": [{"url": "https://mcp-fileserver.internal.example", "name": "primary"}],
            "jwtValidation": {
                "jwksUri": "https://idp.example.com/.well-known/jwks.json",
                "issuer": "https://idp.example.com",
                "audiences": ["mcp-fileserver"]
            },
            "resourceMetadata": {
                "agentgateway.dev/issuer-proxy": "https://gw.example.com/aauth"
            }
        }"#;
        let spec: AgentgatewayBackendSpec = serde_json::from_str(json).expect("parse");
        assert_eq!(spec.targets[0].name.as_deref(), Some("primary"));
        assert_eq!(spec.issuer_proxy(), Some("https://gw.example.com/aauth"));
        let jwt = spec.jwt_validation.unwrap();
        assert_eq!(jwt.audiences, vec!["mcp-fileserver"]);
    }

    #[test]
    fn issuer_proxy_returns_none_when_annotation_missing() {
        let spec = AgentgatewayBackendSpec {
            targets: vec![BackendTarget {
                url: "https://mcp.internal".into(),
                name: None,
            }],
            jwt_validation: None,
            resource_metadata: BTreeMap::new(),
        };
        assert!(spec.issuer_proxy().is_none());
    }
}
