//! `axum::Router` binding for the Person Server, per draft section "Person
//! Server". Endpoint paths are this crate's own choice -- "Metadata
//! Documents" (`aauth-person.json`) is the protocol's actual source of truth
//! for endpoint URLs, which a real deployment discovers rather than
//! hardcodes; the paths below (`/token`, `/permission`, `/audit`,
//! `/interaction`, `/login`) exist so this crate has something concrete to
//! serve and test.
//!
//! HTTP Message Signature verification of the outer request (RFC 9421,
//! `Signature-Key` / `Signature-Input` / `Signature`) is NOT implemented:
//! this module extracts the agent token from `Signature-Key` per the wire
//! format but does not cryptographically verify the request-signing
//! envelope. See the crate-level docs' "Deviations" section.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use trogon_aauth_verify::{JwksResolver, TimeSource};
use trogon_identity_types::aauth::error::ErrorResponse;
use trogon_identity_types::aauth::mission::{MissionBlob, MissionStatus};
use trogon_identity_types::aauth::person_server::{
    AuditRequest, InteractionRequest, PermissionDecision, PermissionRequest, PermissionResponse, TokenRequest,
};

use crate::decision::PolicyEngine;
use crate::error::PersonServerError;
use crate::interaction::InteractionChannel;
use crate::mission::MissionId;
use crate::pending::PendingPhase;
use crate::server::{PersonServer, TokenEndpointOutcome};
use crate::store::PersonStateStore;

/// Header carrying the agent's presentation of its `aa-agent+jwt`, per
/// "Agent Token Request": `Signature-Key: sig=jwt;jwt="<token>"`. Only the
/// embedded token is extracted here; see module docs for the signature
/// verification gap.
const SIGNATURE_KEY_HEADER: &str = "signature-key";

/// Shared axum extractor type for handlers below; factored out to avoid
/// clippy's `type_complexity` lint on the generic `PersonServer` binding.
type PersonServerState<R, C, P, I, S> = State<Arc<PersonServer<R, C, P, I, S>>>;

fn extract_agent_token(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get(SIGNATURE_KEY_HEADER)?.to_str().ok()?;
    let marker = "jwt=\"";
    let start = raw.find(marker)? + marker.len();
    let rest = &raw[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Builds the Person Server's `axum::Router` binding `/token`,
/// `/permission`, `/audit`, `/interaction`, and `/login`.
pub fn router<R, C, P, I, S>(server: Arc<PersonServer<R, C, P, I, S>>) -> Router
where
    R: JwksResolver + Send + Sync + 'static,
    C: TimeSource + Clone + Send + Sync + 'static,
    P: PolicyEngine + Send + Sync + 'static,
    I: InteractionChannel + Send + Sync + 'static,
    S: PersonStateStore + Send + Sync + 'static,
{
    Router::new()
        .route("/token", post(token_endpoint::<R, C, P, I, S>))
        .route("/token/{id}", get(poll_token_endpoint::<R, C, P, I, S>))
        .route("/permission", post(permission_endpoint::<R, C, P, I, S>))
        .route("/audit", post(audit_endpoint::<R, C, P, I, S>))
        .route("/interaction", post(interaction_endpoint::<R, C, P, I, S>))
        .route("/login", get(login_endpoint))
        .with_state(server)
}

fn error_response(err: &PersonServerError) -> Response {
    let status = StatusCode::from_u16(err.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut body = ErrorResponse::new(err.wire_code());
    if let Some(detail) = err.client_detail() {
        body = body.with_detail(detail);
    }
    (status, Json(body)).into_response()
}

async fn token_endpoint<R, C, P, I, S>(
    State(server): PersonServerState<R, C, P, I, S>,
    headers: axum::http::HeaderMap,
    Json(req): Json<TokenRequest>,
) -> Response
where
    R: JwksResolver,
    C: TimeSource + Clone,
    P: PolicyEngine,
    I: InteractionChannel,
    S: PersonStateStore,
{
    let Some(agent_token) = extract_agent_token(&headers) else {
        return error_response(&PersonServerError::Verification(
            crate::error::RequestVerificationError::MissingSignatureKey,
        ));
    };
    match server.evaluate_token_request(&agent_token, &req).await {
        Ok(TokenEndpointOutcome::Grant { response }) => (StatusCode::OK, Json(response)).into_response(),
        Ok(TokenEndpointOutcome::Pending { pending_id, response }) => {
            let location = format!("/token/{}", pending_id.0);
            let headers = [
                (axum::http::header::LOCATION, location),
                (axum::http::header::RETRY_AFTER, "2".to_string()),
            ];
            (StatusCode::ACCEPTED, headers, Json(response)).into_response()
        }
        Err(err) => error_response(&err),
    }
}

async fn poll_token_endpoint<R, C, P, I, S>(
    State(server): PersonServerState<R, C, P, I, S>,
    Path(id): Path<String>,
) -> Response
where
    R: JwksResolver,
    C: TimeSource + Clone,
    P: PolicyEngine,
    I: InteractionChannel,
    S: PersonStateStore,
{
    match server.poll_pending(&crate::pending::PendingId(id.clone())).await {
        Ok(pending) => match pending.phase {
            PendingPhase::Granted { auth_token, expires_in } => (
                StatusCode::OK,
                Json(trogon_identity_types::aauth::person_server::TokenGrantResponse { auth_token, expires_in }),
            )
                .into_response(),
            PendingPhase::Denied { reason } => error_response(&PersonServerError::Denied(reason)),
            PendingPhase::Canceled => (
                StatusCode::GONE,
                Json(ErrorResponse::new("denied").with_detail("request canceled")),
            )
                .into_response(),
            PendingPhase::AwaitingClarification { clarification } => {
                (StatusCode::ACCEPTED, Json(clarification)).into_response()
            }
            PendingPhase::AwaitingResourceInteraction { url, code } => (
                StatusCode::ACCEPTED,
                Json(trogon_identity_types::aauth::person_server::PendingResponse {
                    status: "interacting".to_string(),
                    clarification: None,
                    timeout: None,
                    options: Some(vec![url, code]),
                    required_claims: None,
                }),
            )
                .into_response(),
            PendingPhase::Interacting => (
                StatusCode::ACCEPTED,
                Json(trogon_identity_types::aauth::person_server::PendingResponse {
                    status: "interacting".to_string(),
                    ..trogon_identity_types::aauth::person_server::PendingResponse::pending()
                }),
            )
                .into_response(),
            PendingPhase::Pending | PendingPhase::ApprovalPending => (
                StatusCode::ACCEPTED,
                Json(trogon_identity_types::aauth::person_server::PendingResponse::pending()),
            )
                .into_response(),
        },
        Err(err) => error_response(&err),
    }
}

async fn permission_endpoint<R, C, P, I, S>(
    State(server): PersonServerState<R, C, P, I, S>,
    Json(req): Json<PermissionRequest>,
) -> Response
where
    R: JwksResolver,
    C: TimeSource + Clone,
    P: PolicyEngine,
    I: InteractionChannel,
    S: PersonStateStore,
{
    let Some(mission_ref) = req.mission.clone() else {
        return error_response(&PersonServerError::MissionNotFound(
            crate::mission::MissionId::from_s256("(none provided)"),
        ));
    };
    let mission_id = MissionId::from_s256(mission_ref.s256.clone());
    let entry = trogon_identity_types::aauth::mission::MissionLogEntry::PermissionRequest {
        action: req.action.clone(),
        description: req.description.clone(),
    };
    match server.append_mission_log(&mission_id, entry).await {
        Ok(()) => (
            StatusCode::OK,
            Json(PermissionResponse {
                permission: PermissionDecision::Granted,
                reason: None,
            }),
        )
            .into_response(),
        Err(err) => error_response(&err),
    }
}

async fn audit_endpoint<R, C, P, I, S>(
    State(server): PersonServerState<R, C, P, I, S>,
    Json(req): Json<AuditRequest>,
) -> Response
where
    R: JwksResolver,
    C: TimeSource + Clone,
    P: PolicyEngine,
    I: InteractionChannel,
    S: PersonStateStore,
{
    let mission_id = MissionId::from_s256(req.mission.s256.clone());
    let entry = trogon_identity_types::aauth::mission::MissionLogEntry::AuditRecord {
        action: req.action.clone(),
        description: req.description.clone(),
    };
    match server.append_mission_log(&mission_id, entry).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => error_response(&err),
    }
}

async fn interaction_endpoint<R, C, P, I, S>(
    State(server): PersonServerState<R, C, P, I, S>,
    Json(req): Json<InteractionRequest>,
) -> Response
where
    R: JwksResolver,
    C: TimeSource + Clone,
    P: PolicyEngine,
    I: InteractionChannel,
    S: PersonStateStore,
{
    let Some(mission_ref) = req.mission.clone() else {
        return error_response(&PersonServerError::MissionNotFound(
            crate::mission::MissionId::from_s256("(none provided)"),
        ));
    };
    let mission_id = MissionId::from_s256(mission_ref.s256.clone());
    let type_str = format!("{:?}", req.type_).to_lowercase();
    let entry = trogon_identity_types::aauth::mission::MissionLogEntry::InteractionRequest { type_: type_str };
    match server.append_mission_log(&mission_id, entry).await {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(err) => error_response(&err),
    }
}

async fn login_endpoint(uri: axum::http::Uri) -> Response {
    let query = uri.query().unwrap_or("");
    match crate::login::parse_login_request(query) {
        Some(req) => (StatusCode::OK, Json(req)).into_response(),
        None => (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::new("invalid_request").with_detail("missing required 'ps' query parameter")),
        )
            .into_response(),
    }
}

/// Renders a [`MissionBlob`] approval as its exact wire bytes, exposed so
/// callers building a `mission_endpoint` handler (not wired into
/// [`router`] since mission creation/approval policy is deployment-specific)
/// can serve the byte-exact response "Mission Approval" requires for later
/// `s256` hashing.
pub async fn approve_mission_response<R, C, P, I, S>(
    server: &PersonServer<R, C, P, I, S>,
    blob: MissionBlob,
) -> Result<Vec<u8>, PersonServerError>
where
    R: JwksResolver,
    C: TimeSource + Clone,
    P: PolicyEngine,
    I: InteractionChannel,
    S: PersonStateStore,
{
    server.approve_mission(blob).await
}

/// Renders a [`MissionStatusError`] body for a request referencing a
/// terminated mission, per "Mission Status Errors".
#[must_use]
pub fn mission_status_error_body(status: MissionStatus) -> trogon_identity_types::aauth::mission::MissionStatusError {
    trogon_identity_types::aauth::mission::MissionStatusError {
        error: "mission_terminated".to_string(),
        mission_status: status,
    }
}

#[cfg(test)]
mod tests;
