//! `EnterpriseAgentgatewayPolicy` CRD — Solo.io-shaped token-exchange
//! policy attached to an HTTPRoute or Backend target.
//!
//! Mirrors the spec at
//! `https://docs.solo.io/agentgateway/latest/mcp/token-exchange/` so
//! operators can paste their existing policy verbatim and the
//! controller resolves it to native gateway runtime types.

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "enterpriseagentgateway.solo.io",
    version = "v1alpha1",
    kind = "EnterpriseAgentgatewayPolicy",
    plural = "enterpriseagentgatewaypolicies",
    shortname = "eagwp",
    namespaced,
    status = "EnterpriseAgentgatewayPolicyStatus",
    printcolumn = r#"{"name":"Mode","type":"string","jsonPath":".spec.backend.tokenExchange.mode"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct EnterpriseAgentgatewayPolicySpec {
    /// HTTPRoute / Backend references this policy attaches to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_refs: Vec<PolicyTargetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic: Option<TrafficPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendPolicy>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PolicyTargetRef {
    pub group: String,
    pub kind: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrafficPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwt_authentication: Option<JwtAuthentication>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JwtAuthentication {
    /// `Strict` rejects unauthenticated requests; `Permissive` allows them.
    pub mode: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<JwtProvider>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JwtProvider {
    pub issuer: String,
    pub jwks: JwksSource,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JwksSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackendPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_exchange: Option<TokenExchangePolicy>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TokenExchangePolicy {
    /// `Delegation`, `ExchangeOnly` (impersonation), `ElicitationOnly`,
    /// or `AuthOnly`. Defaults to `Delegation` when omitted.
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entra: Option<EntraConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EntraConfig {
    pub tenant_id: String,
    pub client_id: String,
    pub scope: String,
    pub client_secret_ref: SecretKeyRef,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SecretKeyRef {
    pub name: String,
    pub key: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnterpriseAgentgatewayPolicyStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<EnterpriseAgentgatewayPolicyCondition>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnterpriseAgentgatewayPolicyCondition {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entra_obo_policy_shape() {
        let json = r#"{
            "targetRefs": [{"group":"gateway.networking.k8s.io","kind":"HTTPRoute","name":"mcp"}],
            "backend": {
                "tokenExchange": {
                    "mode": "ExchangeOnly",
                    "entra": {
                        "tenantId": "11111111-2222-3333-4444-555555555555",
                        "clientId": "app-id",
                        "scope": "https://graph.microsoft.com/User.Read",
                        "clientSecretRef": {"name": "entra-secret", "key": "client-secret"}
                    }
                }
            }
        }"#;
        let spec: EnterpriseAgentgatewayPolicySpec = serde_json::from_str(json).expect("parse");
        let entra = spec
            .backend
            .as_ref()
            .and_then(|b| b.token_exchange.as_ref())
            .and_then(|t| t.entra.as_ref())
            .expect("entra block");
        assert_eq!(entra.client_secret_ref.name, "entra-secret");
        assert_eq!(entra.client_secret_ref.key, "client-secret");
    }

    #[test]
    fn parses_elicitation_only_mode() {
        let json = r#"{
            "backend": { "tokenExchange": { "mode": "ElicitationOnly" } }
        }"#;
        let spec: EnterpriseAgentgatewayPolicySpec = serde_json::from_str(json).expect("parse");
        assert_eq!(spec.backend.unwrap().token_exchange.unwrap().mode, "ElicitationOnly");
    }

    #[test]
    fn parses_jwt_authentication_block() {
        let json = r#"{
            "traffic": {
                "jwtAuthentication": {
                    "mode": "Strict",
                    "providers": [{
                        "issuer": "https://idp.example.com",
                        "jwks": {"url": "https://idp.example.com/keys"}
                    }]
                }
            }
        }"#;
        let spec: EnterpriseAgentgatewayPolicySpec = serde_json::from_str(json).expect("parse");
        let jwt = spec.traffic.unwrap().jwt_authentication.unwrap();
        assert_eq!(jwt.mode, "Strict");
        assert_eq!(jwt.providers[0].issuer, "https://idp.example.com");
    }
}
