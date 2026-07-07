//! `axum::Router` binding for the AS token endpoint, per "AS Token Endpoint"
//! (#as-token-endpoint) request/response and "Token Endpoint Error Codes"
//! (#token-endpoint-error-codes) error formats.
//!
//! The PS authenticates the outer HTTP request via an HTTP Message Signature
//! keyed by `Signature-Key: sig=jwks_uri; jwks_uri="..."`
//! (#http-message-signatures-profile), which is a distinct signature profile
//! from this crate's assigned sections. This router therefore expects the
//! verified PS issuer to already be available as a request extension --
//! typically inserted by signature-verification middleware placed in front
//! of this router. [`PsIdentity`] is that extension type.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::post;
use serde::Serialize;
use trogon_identity_types::aauth::error::ErrorResponse;
use trogon_identity_types::aauth::federation::{AsTokenRequest, ClaimsSubmission};

use crate::error::{AccessServerError, RequestVerificationError};
use crate::pending::PendingRequestId;
use crate::policy::OrganizationPolicy;
use crate::server::{AccessServer, AsOutcome};

/// The PS issuer identified by request-signature verification middleware,
/// carried into handlers as an axum `Extension`. Middleware is expected to
/// reject the request before this router runs if the signature does not
/// verify; by the time a handler sees this, `iss` is the PS's authenticated
/// identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PsIdentity {
    pub iss: String,
}

/// Builds the AS token endpoint router: `POST /token` and
/// `POST /token/pending/{id}` (the `Location` URL for `requirement=claims`
/// resumption per "Claims Required").
pub fn router<R, C, P>(server: Arc<AccessServer<R, C, P>>) -> Router
where
    R: trogon_aauth_verify::JwksResolver + Send + Sync + 'static,
    C: trogon_aauth_verify::TimeSource + Clone + Send + Sync + 'static,
    P: OrganizationPolicy + 'static,
{
    Router::new()
        .route("/token", post(token_endpoint::<R, C, P>))
        .route("/token/pending/{id}", post(resume_endpoint::<R, C, P>))
        .with_state(server)
}

async fn token_endpoint<R, C, P>(
    State(server): State<Arc<AccessServer<R, C, P>>>,
    Extension(ps): Extension<PsIdentity>,
    Json(body): Json<AsTokenRequest>,
) -> Response
where
    R: trogon_aauth_verify::JwksResolver,
    C: trogon_aauth_verify::TimeSource + Clone,
    P: OrganizationPolicy,
{
    match server.evaluate(&ps.iss, &body).await {
        Ok(outcome) => outcome_response(outcome),
        Err(err) => error_response(&err),
    }
}

async fn resume_endpoint<R, C, P>(
    State(server): State<Arc<AccessServer<R, C, P>>>,
    Extension(ps): Extension<PsIdentity>,
    Path(id): Path<String>,
    Json(body): Json<ClaimsSubmission>,
) -> Response
where
    R: trogon_aauth_verify::JwksResolver,
    C: trogon_aauth_verify::TimeSource + Clone,
    P: OrganizationPolicy,
{
    let pending_id = PendingRequestId::new(id);
    match server.resume_with_claims(&pending_id, &ps.iss, &body).await {
        Ok(outcome) => outcome_response(outcome),
        Err(err) => error_response(&err),
    }
}

fn outcome_response(outcome: AsOutcome) -> Response {
    match outcome {
        AsOutcome::Issued(resp) => (StatusCode::OK, Json(resp)).into_response(),
        AsOutcome::ClaimsRequired {
            pending_id,
            required_claims,
        } => {
            #[derive(Serialize)]
            struct ClaimsRequiredBody {
                status: &'static str,
                required_claims: Vec<String>,
            }
            let location = format!("/token/pending/{}", pending_id.as_str());
            let mut response = (
                StatusCode::ACCEPTED,
                Json(ClaimsRequiredBody {
                    status: "pending",
                    required_claims,
                }),
            )
                .into_response();
            let headers = response.headers_mut();
            if let Ok(value) = axum::http::HeaderValue::from_str(&location) {
                headers.insert(axum::http::header::LOCATION, value);
            }
            headers.insert(
                "aauth-requirement",
                axum::http::HeaderValue::from_static("requirement=claims"),
            );
            response
        }
        AsOutcome::Denied { reason } => (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse::new("denied").with_detail(reason.as_str())),
        )
            .into_response(),
    }
}

fn error_response(err: &AccessServerError) -> Response {
    let (status, code) = match err {
        AccessServerError::Verification(v) => verification_status(v),
        AccessServerError::Mint(_) => (StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        AccessServerError::Denied { .. } => (StatusCode::FORBIDDEN, "denied"),
        AccessServerError::UnknownPendingRequest(_) => (StatusCode::NOT_FOUND, "invalid_request"),
    };
    (status, Json(ErrorResponse::new(code).with_detail(err.to_string()))).into_response()
}

fn verification_status(err: &RequestVerificationError) -> (StatusCode, &'static str) {
    match err {
        RequestVerificationError::UntrustedPs(_) => (StatusCode::FORBIDDEN, "invalid_request"),
        RequestVerificationError::ResourceToken(_) => (StatusCode::BAD_REQUEST, "invalid_resource_token"),
        RequestVerificationError::AgentToken(_)
        | RequestVerificationError::AgentTokenIsSubagent
        | RequestVerificationError::SubagentToken(_)
        | RequestVerificationError::SubagentParentMismatch { .. }
        | RequestVerificationError::SubagentTokenIsItselfSubagent
        | RequestVerificationError::ResourceTokenSubagentKeyMismatch
        | RequestVerificationError::ResourceTokenSubagentIdentifierMismatch
        | RequestVerificationError::ResourceTokenAgentKeyMismatch
        | RequestVerificationError::ResourceTokenAgentIdentifierMismatch
        | RequestVerificationError::MalformedParentAgentClaim(_) => (StatusCode::BAD_REQUEST, "invalid_agent_token"),
        RequestVerificationError::UpstreamToken(_) | RequestVerificationError::UpstreamUntrustedIssuer(_) => {
            (StatusCode::BAD_REQUEST, "invalid_request")
        }
    }
}

#[cfg(test)]
mod tests;
