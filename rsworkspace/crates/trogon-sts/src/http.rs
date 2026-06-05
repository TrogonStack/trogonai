use std::net::SocketAddr;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{ConnectInfo, Form, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use crate::audit::AuditPublisher;
use crate::cache::JwksCache;
use crate::error::StsError;
use crate::exchange::ExchangeService;
use crate::registry::RegistryLookup;
use crate::spicedb::SpiceDbCheck;
use crate::types::{ExchangeMode, GRANT_TYPE_TOKEN_EXCHANGE, StsExchangeRequest, StsTokenErrorResponse};

#[derive(Debug, Clone, Deserialize)]
pub struct StsExchangeForm {
    pub grant_type: String,
    pub subject_token: String,
    pub subject_token_type: String,
    #[serde(default)]
    pub actor_token: Option<String>,
    #[serde(default)]
    pub actor_token_type: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub requested_token_type: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
}

impl StsExchangeForm {
    fn into_request(self) -> Result<StsExchangeRequest, StsError> {
        if self.grant_type != GRANT_TYPE_TOKEN_EXCHANGE {
            return Err(StsError::InvalidRequest(format!(
                "unsupported grant_type: {}",
                self.grant_type
            )));
        }
        let audience = self
            .audience
            .or(self.resource)
            .ok_or_else(|| StsError::InvalidRequest("audience required".into()))?;
        let requested_token_type = self
            .requested_token_type
            .unwrap_or_else(|| crate::types::TOKEN_TYPE_JWT.to_string());
        let mode = match self.mode.as_deref().unwrap_or("") {
            "" => ExchangeMode::default(),
            raw => ExchangeMode::parse(raw).map_err(StsError::InvalidRequest)?,
        };
        Ok(StsExchangeRequest {
            subject_token: self.subject_token,
            subject_token_type: self.subject_token_type,
            actor_token: self.actor_token.unwrap_or_default(),
            actor_token_type: self
                .actor_token_type
                .unwrap_or_else(|| crate::types::TOKEN_TYPE_JWT.to_string()),
            audience,
            scope: self.scope.unwrap_or_default(),
            purpose: self.purpose.unwrap_or_default(),
            requested_token_type,
            mode,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OAuthDiscovery {
    pub issuer: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    pub grant_types_supported: Vec<String>,
    pub token_endpoint_auth_methods_supported: Vec<String>,
    pub subject_token_types_supported: Vec<String>,
    pub actor_token_types_supported: Vec<String>,
    pub response_types_supported: Vec<String>,
}

pub struct StsHttpState<R, A, S>
where
    R: RegistryLookup + Clone + Send + Sync + 'static,
    A: AuditPublisher + Send + Sync + 'static,
    S: SpiceDbCheck + Send + Sync + 'static,
{
    pub service: Arc<ExchangeService<R, A, S>>,
    pub jwks: JwksCache,
    pub issuer: String,
    pub external_base_url: String,
}

impl<R, A, S> Clone for StsHttpState<R, A, S>
where
    R: RegistryLookup + Clone + Send + Sync + 'static,
    A: AuditPublisher + Send + Sync + 'static,
    S: SpiceDbCheck + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            service: Arc::clone(&self.service),
            jwks: self.jwks.clone(),
            issuer: self.issuer.clone(),
            external_base_url: self.external_base_url.clone(),
        }
    }
}

pub fn router<R, A, S>(state: StsHttpState<R, A, S>) -> Router
where
    R: RegistryLookup + Clone + Send + Sync + 'static,
    A: AuditPublisher + Send + Sync + 'static,
    S: SpiceDbCheck + Send + Sync + 'static,
{
    Router::new()
        .route("/oauth2/token", post(post_token::<R, A, S>))
        .route("/.well-known/oauth-authorization-server", get(get_discovery::<R, A, S>))
        .route("/.well-known/jwks.json", get(get_jwks::<R, A, S>))
        .route("/healthz", get(healthz))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn post_token<R, A, S>(
    State(state): State<StsHttpState<R, A, S>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Form(form): Form<StsExchangeForm>,
) -> Response
where
    R: RegistryLookup + Clone + Send + Sync + 'static,
    A: AuditPublisher + Send + Sync + 'static,
    S: SpiceDbCheck + Send + Sync + 'static,
{
    let request = match form.into_request() {
        Ok(r) => r,
        Err(err) => return error_response(&err).into_response(),
    };
    match state.service.handle(request, Some(addr.ip().to_string())).await {
        Ok(ok) => (StatusCode::OK, Json(ok)).into_response(),
        Err(err) => {
            let status = status_for_error(&err.error);
            (status, Json(err)).into_response()
        }
    }
}

async fn get_discovery<R, A, S>(State(state): State<StsHttpState<R, A, S>>) -> Json<OAuthDiscovery>
where
    R: RegistryLookup + Clone + Send + Sync + 'static,
    A: AuditPublisher + Send + Sync + 'static,
    S: SpiceDbCheck + Send + Sync + 'static,
{
    let base = state.external_base_url.trim_end_matches('/');
    Json(OAuthDiscovery {
        issuer: state.issuer.clone(),
        token_endpoint: format!("{base}/oauth2/token"),
        jwks_uri: format!("{base}/.well-known/jwks.json"),
        grant_types_supported: vec![GRANT_TYPE_TOKEN_EXCHANGE.to_string()],
        token_endpoint_auth_methods_supported: vec!["none".to_string()],
        subject_token_types_supported: vec![crate::types::TOKEN_TYPE_JWT.to_string()],
        actor_token_types_supported: vec![crate::types::TOKEN_TYPE_JWT.to_string()],
        response_types_supported: vec![],
    })
}

async fn get_jwks<R, A, S>(State(state): State<StsHttpState<R, A, S>>) -> Json<serde_json::Value>
where
    R: RegistryLookup + Clone + Send + Sync + 'static,
    A: AuditPublisher + Send + Sync + 'static,
    S: SpiceDbCheck + Send + Sync + 'static,
{
    let mesh = state.jwks.mesh_jwks().await;
    Json(serde_json::to_value(&mesh).unwrap_or_else(|_| serde_json::json!({"keys": []})))
}

fn error_response(err: &StsError) -> (StatusCode, Json<StsTokenErrorResponse>) {
    let body = StsTokenErrorResponse::from_sts_error(err);
    let status = status_for_error(&body.error);
    (status, Json(body))
}

fn status_for_error(code: &str) -> StatusCode {
    match code {
        "invalid_request"
        | "invalid_grant"
        | "invalid_target"
        | "act_chain_depth_exceeded"
        | "act_chain_loop_detected"
        | "act_chain_entry_revoked" => StatusCode::BAD_REQUEST,
        "access_denied" => StatusCode::FORBIDDEN,
        "rate_limited" => StatusCode::TOO_MANY_REQUESTS,
        "server_error" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_to_request_round_trip() {
        let form = StsExchangeForm {
            grant_type: GRANT_TYPE_TOKEN_EXCHANGE.to_string(),
            subject_token: "subject.jwt".into(),
            subject_token_type: crate::types::TOKEN_TYPE_JWT.into(),
            actor_token: Some("actor.jwt".into()),
            actor_token_type: None,
            audience: Some("aud".into()),
            resource: None,
            scope: Some("tools:read".into()),
            purpose: Some("test".into()),
            requested_token_type: None,
            mode: None,
        };
        let req = form.into_request().unwrap();
        assert_eq!(req.subject_token, "subject.jwt");
        assert_eq!(req.actor_token_type, crate::types::TOKEN_TYPE_JWT);
        assert_eq!(req.audience, "aud");
        assert_eq!(req.requested_token_type, crate::types::TOKEN_TYPE_JWT);
        assert_eq!(req.mode, ExchangeMode::Delegation);
    }

    #[test]
    fn form_parses_exchange_only_mode() {
        let form = StsExchangeForm {
            grant_type: GRANT_TYPE_TOKEN_EXCHANGE.into(),
            subject_token: "s".into(),
            subject_token_type: crate::types::TOKEN_TYPE_JWT.into(),
            actor_token: None,
            actor_token_type: None,
            audience: Some("a".into()),
            resource: None,
            scope: None,
            purpose: None,
            requested_token_type: None,
            mode: Some("ExchangeOnly".into()),
        };
        let req = form.into_request().unwrap();
        assert_eq!(req.mode, ExchangeMode::ExchangeOnly);
    }

    #[test]
    fn form_parses_elicitation_only_mode() {
        let form = StsExchangeForm {
            grant_type: GRANT_TYPE_TOKEN_EXCHANGE.into(),
            subject_token: "s".into(),
            subject_token_type: crate::types::TOKEN_TYPE_JWT.into(),
            actor_token: None,
            actor_token_type: None,
            audience: Some("a".into()),
            resource: None,
            scope: None,
            purpose: None,
            requested_token_type: None,
            mode: Some("ElicitationOnly".into()),
        };
        let req = form.into_request().unwrap();
        assert_eq!(req.mode, ExchangeMode::ElicitationOnly);
    }

    #[test]
    fn form_parses_auth_only_mode() {
        let form = StsExchangeForm {
            grant_type: GRANT_TYPE_TOKEN_EXCHANGE.into(),
            subject_token: "s".into(),
            subject_token_type: crate::types::TOKEN_TYPE_JWT.into(),
            actor_token: None,
            actor_token_type: None,
            audience: Some("a".into()),
            resource: None,
            scope: None,
            purpose: None,
            requested_token_type: None,
            mode: Some("AuthOnly".into()),
        };
        let req = form.into_request().unwrap();
        assert_eq!(req.mode, ExchangeMode::AuthOnly);
    }

    #[test]
    fn form_rejects_unsupported_grant() {
        let form = StsExchangeForm {
            grant_type: "client_credentials".into(),
            subject_token: "x".into(),
            subject_token_type: crate::types::TOKEN_TYPE_JWT.into(),
            actor_token: None,
            actor_token_type: None,
            audience: Some("a".into()),
            resource: None,
            scope: None,
            purpose: None,
            requested_token_type: None,
            mode: None,
        };
        let err = form.into_request().unwrap_err();
        assert!(matches!(err, StsError::InvalidRequest(_)));
    }

    #[test]
    fn resource_aliases_audience() {
        let form = StsExchangeForm {
            grant_type: GRANT_TYPE_TOKEN_EXCHANGE.into(),
            subject_token: "x".into(),
            subject_token_type: crate::types::TOKEN_TYPE_JWT.into(),
            actor_token: None,
            actor_token_type: None,
            audience: None,
            resource: Some("https://api.example.com".into()),
            scope: None,
            purpose: None,
            requested_token_type: None,
            mode: None,
        };
        let req = form.into_request().unwrap();
        assert_eq!(req.audience, "https://api.example.com");
    }

    #[test]
    fn error_status_mapping() {
        assert_eq!(status_for_error("invalid_request"), StatusCode::BAD_REQUEST);
        assert_eq!(status_for_error("access_denied"), StatusCode::FORBIDDEN);
        assert_eq!(status_for_error("rate_limited"), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(status_for_error("server_error"), StatusCode::SERVICE_UNAVAILABLE);
    }
}
