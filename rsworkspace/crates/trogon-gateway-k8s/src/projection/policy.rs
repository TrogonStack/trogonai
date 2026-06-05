//! `EnterpriseAgentgatewayPolicy` → KV projection.

use kube::ResourceExt;
use serde::{Deserialize, Serialize};

use crate::crd::{
    EnterpriseAgentgatewayPolicy, EnterpriseAgentgatewayPolicyCondition, EnterpriseAgentgatewayPolicyStatus,
};

use super::gateway_config::{ProjectionError, content_hash_hex};

pub fn enterprise_policy_kv_key(namespace: &str, name: &str) -> String {
    format!("mcp.gateway.policy.{namespace}.{name}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EnterprisePolicyKvValue {
    pub target_refs: Vec<PolicyTargetKv>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jwt_authentication: Option<JwtAuthenticationKv>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_exchange: Option<TokenExchangeKv>,
    pub source: String,
    pub source_uid: String,
    pub source_generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PolicyTargetKv {
    pub group: String,
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JwtAuthenticationKv {
    pub mode: String,
    pub providers: Vec<JwtProviderKv>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JwtProviderKv {
    pub issuer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jwks_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jwks_inline: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TokenExchangeKv {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entra: Option<EntraKv>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EntraKv {
    pub tenant_id: String,
    pub client_id: String,
    pub scope: String,
    pub client_secret_name: String,
    pub client_secret_key: String,
}

#[derive(Debug)]
pub struct EnterprisePolicyProjection {
    pub key: String,
    pub value: EnterprisePolicyKvValue,
    pub content_hash: String,
}

pub fn project_enterprise_policy(
    resource: &EnterpriseAgentgatewayPolicy,
) -> Result<EnterprisePolicyProjection, ProjectionError> {
    let namespace = resource.namespace().ok_or(ProjectionError::MissingNamespace)?;
    let name = resource.name_any();
    if name.is_empty() {
        return Err(ProjectionError::MissingName);
    }

    let target_refs = resource
        .spec
        .target_refs
        .iter()
        .map(|r| PolicyTargetKv {
            group: r.group.clone(),
            kind: r.kind.clone(),
            name: r.name.clone(),
        })
        .collect();

    let jwt_authentication = resource
        .spec
        .traffic
        .as_ref()
        .and_then(|t| t.jwt_authentication.as_ref())
        .map(|jwt| JwtAuthenticationKv {
            mode: jwt.mode.clone(),
            providers: jwt
                .providers
                .iter()
                .map(|p| JwtProviderKv {
                    issuer: p.issuer.clone(),
                    jwks_url: p.jwks.url.clone(),
                    jwks_inline: p.jwks.inline.clone(),
                })
                .collect(),
        });

    let token_exchange = resource
        .spec
        .backend
        .as_ref()
        .and_then(|b| b.token_exchange.as_ref())
        .map(|tx| TokenExchangeKv {
            mode: tx.mode.clone(),
            entra: tx.entra.as_ref().map(|e| EntraKv {
                tenant_id: e.tenant_id.clone(),
                client_id: e.client_id.clone(),
                scope: e.scope.clone(),
                client_secret_name: e.client_secret_ref.name.clone(),
                client_secret_key: e.client_secret_ref.key.clone(),
            }),
        });

    let value = EnterprisePolicyKvValue {
        target_refs,
        jwt_authentication,
        token_exchange,
        source: "k8s".to_string(),
        source_uid: resource.metadata.uid.clone().unwrap_or_else(|| "unknown".to_string()),
        source_generation: resource.metadata.generation.unwrap_or(0),
    };

    let bytes = serde_json::to_vec(&value).map_err(|e| ProjectionError::Json(e.to_string()))?;
    Ok(EnterprisePolicyProjection {
        key: enterprise_policy_kv_key(&namespace, &name),
        content_hash: content_hash_hex(&bytes),
        value,
    })
}

pub fn build_enterprise_policy_status(
    resource: &EnterpriseAgentgatewayPolicy,
    kv_revision: u64,
) -> EnterpriseAgentgatewayPolicyStatus {
    let generation = resource.metadata.generation.unwrap_or(0);
    let now = chrono_like_now();
    EnterpriseAgentgatewayPolicyStatus {
        observed_generation: Some(generation),
        conditions: vec![
            EnterpriseAgentgatewayPolicyCondition {
                condition_type: "Ready".to_string(),
                status: "True".to_string(),
                reason: Some("Synced".to_string()),
                message: Some(format!("KV revision {kv_revision}")),
                last_transition_time: Some(now.clone()),
            },
            EnterpriseAgentgatewayPolicyCondition {
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
    use crate::crd::{
        BackendPolicy, EnterpriseAgentgatewayPolicySpec, EntraConfig, JwksSource, JwtAuthentication, JwtProvider,
        PolicyTargetRef, SecretKeyRef, TokenExchangePolicy, TrafficPolicy,
    };
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn sample() -> EnterpriseAgentgatewayPolicy {
        EnterpriseAgentgatewayPolicy {
            metadata: ObjectMeta {
                name: Some("graph-obo".to_string()),
                namespace: Some("tenant-acme".to_string()),
                uid: Some("uid-1".to_string()),
                generation: Some(3),
                ..Default::default()
            },
            spec: EnterpriseAgentgatewayPolicySpec {
                target_refs: vec![PolicyTargetRef {
                    group: "gateway.networking.k8s.io".into(),
                    kind: "HTTPRoute".into(),
                    name: "mcp".into(),
                }],
                traffic: Some(TrafficPolicy {
                    jwt_authentication: Some(JwtAuthentication {
                        mode: "Strict".into(),
                        providers: vec![JwtProvider {
                            issuer: "https://idp.example.com".into(),
                            jwks: JwksSource {
                                url: Some("https://idp.example.com/keys".into()),
                                inline: None,
                            },
                        }],
                    }),
                }),
                backend: Some(BackendPolicy {
                    token_exchange: Some(TokenExchangePolicy {
                        mode: "ExchangeOnly".into(),
                        entra: Some(EntraConfig {
                            tenant_id: "tid".into(),
                            client_id: "cid".into(),
                            scope: "Graph.Read".into(),
                            client_secret_ref: SecretKeyRef {
                                name: "entra-secret".into(),
                                key: "client-secret".into(),
                            },
                        }),
                    }),
                }),
            },
            status: None,
        }
    }

    #[test]
    fn projects_jwt_and_token_exchange() {
        let proj = project_enterprise_policy(&sample()).expect("project");
        assert_eq!(proj.key, "mcp.gateway.policy.tenant-acme.graph-obo");
        let jwt = proj.value.jwt_authentication.as_ref().expect("jwt");
        assert_eq!(jwt.mode, "Strict");
        assert_eq!(jwt.providers[0].issuer, "https://idp.example.com");
        let tx = proj.value.token_exchange.as_ref().expect("tx");
        assert_eq!(tx.mode, "ExchangeOnly");
        assert_eq!(tx.entra.as_ref().unwrap().client_secret_name, "entra-secret");
    }

    #[test]
    fn status_includes_observed_generation() {
        let s = build_enterprise_policy_status(&sample(), 42);
        assert_eq!(s.observed_generation, Some(3));
        assert!(
            s.conditions
                .iter()
                .any(|c| c.condition_type == "Ready" && c.status == "True")
        );
        assert!(s.conditions.iter().any(|c| c.condition_type == "Synced"));
    }
}
