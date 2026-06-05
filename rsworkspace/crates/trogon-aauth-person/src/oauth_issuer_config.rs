//! `KGW_OAUTH_ISSUER_CONFIG` — gateway-wide OAuth issuer settings.
//!
//! Mirrors Solo.io's agentgateway consent-screen configuration shape so
//! operators can copy values verbatim. The env var holds JSON whose
//! top-level keys are `gateway_config`, `downstream_server`, and an
//! optional `consent` block that drives consent-page branding.

use std::env;

use serde::{Deserialize, Serialize};

pub const ENV_KEY: &str = "KGW_OAUTH_ISSUER_CONFIG";

#[derive(Debug, thiserror::Error)]
pub enum OAuthIssuerConfigError {
    #[error("env var {0} is not set")]
    Missing(&'static str),
    #[error("invalid {0} JSON: {1}")]
    InvalidJson(&'static str, serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayConfig {
    pub base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownstreamServer {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ConsentBranding {
    pub platform_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthIssuerConfig {
    pub gateway_config: GatewayConfig,
    pub downstream_server: DownstreamServer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<ConsentBranding>,
}

impl OAuthIssuerConfig {
    pub fn from_env() -> Result<Self, OAuthIssuerConfigError> {
        let raw = env::var(ENV_KEY).map_err(|_| OAuthIssuerConfigError::Missing(ENV_KEY))?;
        Self::from_json(&raw)
    }

    pub fn from_json(raw: &str) -> Result<Self, OAuthIssuerConfigError> {
        serde_json::from_str(raw).map_err(|e| OAuthIssuerConfigError::InvalidJson(ENV_KEY, e))
    }

    pub fn consent_branding(&self) -> ConsentBranding {
        self.consent.clone().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config_with_consent_block() {
        let raw = r#"{
            "gateway_config": {"base_url": "https://gw.example.com"},
            "downstream_server": {
                "issuer": "https://idp.example.com",
                "authorization_endpoint": "https://idp.example.com/authorize",
                "token_endpoint": "https://idp.example.com/token",
                "jwks_uri": "https://idp.example.com/.well-known/jwks.json",
                "client_id": "gw",
                "client_secret": "shh"
            },
            "consent": {
                "platform_name": "Acme Platform",
                "logo_url": "https://cdn.example.com/logo.png",
                "legal_text": "By continuing you agree to Acme's terms."
            }
        }"#;
        let cfg = OAuthIssuerConfig::from_json(raw).expect("parse");
        assert_eq!(cfg.gateway_config.base_url, "https://gw.example.com");
        assert_eq!(cfg.downstream_server.client_id.as_deref(), Some("gw"));
        let branding = cfg.consent_branding();
        assert_eq!(branding.platform_name, "Acme Platform");
        assert_eq!(
            branding.logo_url.as_deref(),
            Some("https://cdn.example.com/logo.png")
        );
        assert!(branding.legal_text.is_some());
    }

    #[test]
    fn consent_block_is_optional() {
        let raw = r#"{
            "gateway_config": {"base_url": "https://gw.example.com"},
            "downstream_server": {
                "issuer": "https://idp.example.com",
                "authorization_endpoint": "https://idp.example.com/authorize",
                "token_endpoint": "https://idp.example.com/token",
                "jwks_uri": "https://idp.example.com/.well-known/jwks.json"
            }
        }"#;
        let cfg = OAuthIssuerConfig::from_json(raw).expect("parse");
        assert!(cfg.consent.is_none());
        assert_eq!(cfg.consent_branding(), ConsentBranding::default());
    }

    #[test]
    fn rejects_invalid_json() {
        let err = OAuthIssuerConfig::from_json("not-json").unwrap_err();
        assert!(matches!(err, OAuthIssuerConfigError::InvalidJson(_, _)));
    }
}
