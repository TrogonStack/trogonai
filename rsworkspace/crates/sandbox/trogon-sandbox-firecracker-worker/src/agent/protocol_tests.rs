//! The guest protocol handshake from the worker's side: the version header goes out on
//! every call, and a guest that is too old (or silent about its version) is refused.

use anyhow::bail;
use axum::Router;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use trogon_sandbox_actor_agent::{
    BIND_PATH, BindLeaseRequest, GUEST_PROTOCOL_VERSION, GuestProtocolError, IDENTITY_PATH, READYZ_PATH,
    WORKER_PROTOCOL_VERSION_HEADER,
};
use trogon_sandbox_contract::LeaseId;

use super::tests::{serve, serve_unversioned};
use super::*;

fn actor() -> serde_json::Value {
    serde_json::json!({"actor_id": "actor-1"})
}

fn identity_router() -> Router {
    Router::new().route(IDENTITY_PATH, get(|| async { axum::Json(actor()) }))
}

fn bind_request() -> BindLeaseRequest {
    BindLeaseRequest {
        entropy_seed: None,
        lease_id: LeaseId::new("01J0"),
        env: Default::default(),
        working_dir: None,
        exposed_port: None,
    }
}

#[tokio::test]
async fn every_call_carries_the_workers_protocol_version() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(
        Router::new()
            .route(
                IDENTITY_PATH,
                get(|headers: HeaderMap| async move {
                    assert_eq!(
                        headers[WORKER_PROTOCOL_VERSION_HEADER],
                        GUEST_PROTOCOL_VERSION.to_string().as_str()
                    );
                    axum::Json(actor())
                }),
            )
            .route(
                BIND_PATH,
                post(|headers: HeaderMap| async move {
                    assert_eq!(
                        headers[WORKER_PROTOCOL_VERSION_HEADER],
                        GUEST_PROTOCOL_VERSION.to_string().as_str()
                    );
                    axum::http::StatusCode::NO_CONTENT
                }),
            ),
    )
    .await?;

    assert_eq!(agent.identity().await?.actor_id.as_str(), "actor-1");
    agent.bind(&bind_request()).await?;
    Ok(())
}

#[tokio::test]
async fn a_guest_that_sends_no_version_is_refused_by_default() -> anyhow::Result<()> {
    let (agent, _tmp) = serve_unversioned(identity_router()).await?;

    match agent.identity().await {
        Err(AgentError::ProtocolMismatch { reason, .. }) => {
            assert_eq!(
                reason,
                GuestProtocolError::VersionTooOld {
                    peer_version: 0,
                    min_supported: MIN_SUPPORTED_AGENT_PROTOCOL_VERSION,
                    this_version: GUEST_PROTOCOL_VERSION,
                }
            );
            Ok(())
        }
        other => bail!("got {other:?}"),
    }
}

#[tokio::test]
async fn the_override_admits_a_guest_that_predates_the_handshake() -> anyhow::Result<()> {
    let (agent, _tmp) = serve_unversioned(identity_router()).await?;
    let agent = agent.with_min_agent_protocol_version(0);

    assert_eq!(agent.identity().await?.actor_id.as_str(), "actor-1");
    Ok(())
}

#[tokio::test]
async fn a_guest_below_a_raised_minimum_is_refused() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(identity_router()).await?;
    let agent = agent.with_min_agent_protocol_version(GUEST_PROTOCOL_VERSION + 1);

    match agent.identity().await {
        Err(AgentError::ProtocolMismatch { reason, .. }) => {
            assert_eq!(
                reason,
                GuestProtocolError::VersionTooOld {
                    peer_version: GUEST_PROTOCOL_VERSION,
                    min_supported: GUEST_PROTOCOL_VERSION + 1,
                    this_version: GUEST_PROTOCOL_VERSION,
                }
            );
            Ok(())
        }
        other => bail!("got {other:?}"),
    }
}

#[tokio::test]
async fn a_typed_426_from_the_guest_is_a_protocol_mismatch_not_a_plain_http_error() -> anyhow::Result<()> {
    let refusal = GuestProtocolError::VersionTooOld {
        peer_version: 1,
        min_supported: 2,
        this_version: 2,
    };
    let body = refusal.clone();
    let (agent, _tmp) = serve(Router::new().route(
        BIND_PATH,
        post(move || {
            let body = body.clone();
            async move { (axum::http::StatusCode::UPGRADE_REQUIRED, axum::Json(body)) }
        }),
    ))
    .await?;

    match agent.bind(&bind_request()).await {
        Err(AgentError::ProtocolMismatch { reason, .. }) => {
            assert_eq!(reason, refusal);
            Ok(())
        }
        other => bail!("got {other:?}"),
    }
}

#[tokio::test]
async fn readiness_gives_up_at_once_on_a_protocol_mismatch() -> anyhow::Result<()> {
    let (agent, _tmp) =
        serve_unversioned(Router::new().route(READYZ_PATH, get(|| async { axum::http::StatusCode::OK }))).await?;

    let started = tokio::time::Instant::now();
    let result = agent.wait_ready(std::time::Duration::from_secs(20)).await;
    assert!(matches!(result, Err(AgentError::ProtocolMismatch { .. })), "{result:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    Ok(())
}
