//! Spec-shaped validator configuration.
//!
//! Solo.io agentgateway's Helm values configure subject/actor/api
//! validators as:
//!
//! ```yaml
//! subjectValidator:
//!   validatorType: remote
//!   remoteConfig:
//!     url: "https://idp.example.com/.well-known/jwks.json"
//! actorValidator:
//!   validatorType: k8s
//! ```
//!
//! This module models that shape so operators can paste their existing
//! Helm config and the gateway resolves to the right concrete
//! attestor/JWKS resolver behind the scenes.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "validatorType", rename_all = "lowercase")]
pub enum ValidatorConfig {
    /// Validate JWTs against a remote IdP's JWKS endpoint.
    Remote {
        #[serde(rename = "remoteConfig")]
        remote_config: RemoteConfig,
    },
    /// Validate Kubernetes projected service-account tokens.
    K8s {
        #[serde(default, rename = "k8sConfig")]
        k8s_config: Option<K8sConfig>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audiences: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct K8sConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_domain: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audiences: Vec<String>,
}

/// The Helm-shaped validator block: subject + actor + optional api.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorsConfig {
    #[serde(rename = "subjectValidator")]
    pub subject_validator: ValidatorConfig,
    #[serde(rename = "actorValidator")]
    pub actor_validator: ValidatorConfig,
    /// Only present for Entra/external-IdP topologies. Mirrors
    /// `subjectValidator` but applied to the API token leg.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "apiValidator")]
    pub api_validator: Option<ValidatorConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remote_subject_k8s_actor() {
        let json = r#"{
            "subjectValidator": {
                "validatorType": "remote",
                "remoteConfig": {
                    "url": "https://keycloak/realms/r/protocol/openid-connect/certs",
                    "issuer": "https://keycloak/realms/r"
                }
            },
            "actorValidator": { "validatorType": "k8s" }
        }"#;
        let cfg: ValidatorsConfig = serde_json::from_str(json).expect("parse");
        match cfg.subject_validator {
            ValidatorConfig::Remote { remote_config } => {
                assert!(remote_config.url.ends_with("/certs"));
                assert_eq!(remote_config.issuer.as_deref(), Some("https://keycloak/realms/r"));
            }
            _ => panic!("subject should be remote"),
        }
        assert!(matches!(cfg.actor_validator, ValidatorConfig::K8s { .. }));
        assert!(cfg.api_validator.is_none());
    }

    #[test]
    fn parses_entra_shape_with_api_validator() {
        let json = r#"{
            "subjectValidator": {
                "validatorType": "remote",
                "remoteConfig": {
                    "url": "https://login.microsoftonline.com/T/discovery/v2.0/keys"
                }
            },
            "actorValidator": { "validatorType": "k8s" },
            "apiValidator": {
                "validatorType": "remote",
                "remoteConfig": {
                    "url": "https://login.microsoftonline.com/T/discovery/v2.0/keys"
                }
            }
        }"#;
        let cfg: ValidatorsConfig = serde_json::from_str(json).expect("parse");
        assert!(cfg.api_validator.is_some());
    }
}
