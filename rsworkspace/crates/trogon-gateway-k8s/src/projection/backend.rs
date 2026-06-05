//! `AgentgatewayBackend` → KV projection.

use std::collections::BTreeMap;

use kube::ResourceExt;
use serde::{Deserialize, Serialize};

use crate::crd::{AgentgatewayBackend, AgentgatewayBackendStatus, BackendCondition, ISSUER_PROXY_ANNOTATION};

use super::gateway_config::{ProjectionError, content_hash_hex};

pub fn agentgateway_backend_kv_key(namespace: &str, name: &str) -> String {
    format!("mcp.gateway.backend.{namespace}.{name}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentgatewayBackendKvValue {
    pub targets: Vec<BackendTargetKv>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jwt_validation: Option<JwtValidationKv>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer_proxy: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub resource_metadata: BTreeMap<String, String>,
    pub source: String,
    pub source_uid: String,
    pub source_generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendTargetKv {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JwtValidationKv {
    pub jwks_uri: String,
    pub issuer: String,
    pub audiences: Vec<String>,
}

#[derive(Debug)]
pub struct AgentgatewayBackendProjection {
    pub key: String,
    pub value: AgentgatewayBackendKvValue,
    pub content_hash: String,
}

pub fn project_agentgateway_backend(
    resource: &AgentgatewayBackend,
) -> Result<AgentgatewayBackendProjection, ProjectionError> {
    let namespace = resource.namespace().ok_or(ProjectionError::MissingNamespace)?;
    let name = resource.name_any();
    if name.is_empty() {
        return Err(ProjectionError::MissingName);
    }

    let targets = resource
        .spec
        .targets
        .iter()
        .map(|t| BackendTargetKv {
            url: t.url.clone(),
            name: t.name.clone(),
        })
        .collect();

    let jwt_validation = resource.spec.jwt_validation.as_ref().map(|j| JwtValidationKv {
        jwks_uri: j.jwks_uri.clone(),
        issuer: j.issuer.clone(),
        audiences: j.audiences.clone(),
    });

    let issuer_proxy = resource.spec.resource_metadata.get(ISSUER_PROXY_ANNOTATION).cloned();

    let value = AgentgatewayBackendKvValue {
        targets,
        jwt_validation,
        issuer_proxy,
        resource_metadata: resource.spec.resource_metadata.clone(),
        source: "k8s".to_string(),
        source_uid: resource.metadata.uid.clone().unwrap_or_else(|| "unknown".to_string()),
        source_generation: resource.metadata.generation.unwrap_or(0),
    };

    let bytes = serde_json::to_vec(&value).map_err(|e| ProjectionError::Json(e.to_string()))?;
    Ok(AgentgatewayBackendProjection {
        key: agentgateway_backend_kv_key(&namespace, &name),
        content_hash: content_hash_hex(&bytes),
        value,
    })
}

pub fn build_agentgateway_backend_status(
    resource: &AgentgatewayBackend,
    kv_revision: u64,
) -> AgentgatewayBackendStatus {
    let generation = resource.metadata.generation.unwrap_or(0);
    let now = chrono_like_now();
    AgentgatewayBackendStatus {
        observed_generation: Some(generation),
        conditions: vec![
            BackendCondition {
                condition_type: "Ready".to_string(),
                status: "True".to_string(),
                reason: Some("Synced".to_string()),
                message: Some(format!("KV revision {kv_revision}")),
                last_transition_time: Some(now.clone()),
            },
            BackendCondition {
                condition_type: "Synced".to_string(),
                status: "True".to_string(),
                reason: Some("KvWriteSucceeded".to_string()),
                message: None,
                last_transition_time: Some(now),
            },
        ],
    }
}

fn chrono_like_now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::{AgentgatewayBackendSpec, BackendTarget, JwtValidation};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn sample() -> AgentgatewayBackend {
        let mut resource_metadata = BTreeMap::new();
        resource_metadata.insert(
            ISSUER_PROXY_ANNOTATION.to_string(),
            "https://gw.example.com/aauth".to_string(),
        );
        AgentgatewayBackend {
            metadata: ObjectMeta {
                name: Some("mcp-fileserver".to_string()),
                namespace: Some("tenant-acme".to_string()),
                uid: Some("uid-7".to_string()),
                generation: Some(2),
                ..Default::default()
            },
            spec: AgentgatewayBackendSpec {
                targets: vec![BackendTarget {
                    url: "https://mcp-fileserver.internal.example".into(),
                    name: Some("primary".into()),
                }],
                jwt_validation: Some(JwtValidation {
                    jwks_uri: "https://idp.example.com/.well-known/jwks.json".into(),
                    issuer: "https://idp.example.com".into(),
                    audiences: vec!["mcp-fileserver".into()],
                }),
                resource_metadata,
            },
            status: None,
        }
    }

    #[test]
    fn projects_jwt_validation_and_issuer_proxy() {
        let p = project_agentgateway_backend(&sample()).expect("project");
        assert_eq!(p.key, "mcp.gateway.backend.tenant-acme.mcp-fileserver");
        assert_eq!(p.value.issuer_proxy.as_deref(), Some("https://gw.example.com/aauth"));
        let jwt = p.value.jwt_validation.as_ref().expect("jwt");
        assert_eq!(jwt.audiences, vec!["mcp-fileserver".to_string()]);
    }

    #[test]
    fn status_includes_observed_generation() {
        let s = build_agentgateway_backend_status(&sample(), 7);
        assert_eq!(s.observed_generation, Some(2));
        assert!(s.conditions.iter().any(|c| c.condition_type == "Ready"));
    }
}
