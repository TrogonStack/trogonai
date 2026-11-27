//! Shared fixtures for the agent transport tests, also used by the worker and fleet tests.

use std::error::Error;

use axum::body::Body;
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::response::Response as AxumResponse;
use serde::de::DeserializeOwned;
use trogon_sandbox_contract::{LeaseId, RunCommandRequest, TenantId};

use super::{ActorAgent, ActorTarget};
use crate::config::RouterEndpoint;
use crate::naming::{ActorName, Atespace};

mod exec_tests;
mod fake_router;
mod protocol_tests;
mod transport_tests;
mod verb_tests;

pub(crate) use fake_router::{serve, serve_raw_reply, serve_unversioned};

pub(crate) fn target() -> Result<ActorTarget, Box<dyn Error>> {
    target_for_tenant("tenant-a")
}

pub(crate) fn target_for_tenant(tenant: &str) -> Result<ActorTarget, Box<dyn Error>> {
    let atespace = Atespace::for_tenant(&TenantId::new(tenant)?)?;
    let actor = ActorName::for_lease(&LeaseId::new("01J0"))?;
    Ok(ActorTarget::new(atespace, actor, 8110))
}

/// A newline-delimited body, as the exec endpoint answers.
pub(crate) fn ndjson(body: &'static str) -> AxumResponse {
    let mut response = AxumResponse::new(Body::from(body));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/x-ndjson"));
    response
}

/// An attached command that streams its output.
pub(crate) fn request() -> Result<RunCommandRequest, Box<dyn Error>> {
    parse(r#"{"lease_id":"01J0","command":["true"],"timeout_seconds":5,"stream_output":true}"#)
}

pub(crate) fn agent_router(agent: &ActorAgent) -> RouterEndpoint {
    agent.router().clone()
}

pub(crate) fn parse<T: DeserializeOwned>(json: &str) -> Result<T, Box<dyn Error>> {
    Ok(serde_json::from_str(json)?)
}
