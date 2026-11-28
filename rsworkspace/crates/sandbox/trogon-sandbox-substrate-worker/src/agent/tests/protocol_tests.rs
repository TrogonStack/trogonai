//! The guest protocol handshake from the worker's side: the version header goes out on
//! every call, and an agent that is too old (or silent about its version) is refused.

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use trogon_sandbox_actor_agent::{
    ActorId, ActorIdentity, BIND_PATH, BindLeaseRequest, GUEST_PROTOCOL_VERSION, GuestProtocolError, IDENTITY_PATH,
    MIN_AGENT_PROTOCOL_VERSION_ENV, MIN_SUPPORTED_AGENT_PROTOCOL_VERSION, READYZ_PATH, WORKER_PROTOCOL_VERSION_HEADER,
};
use trogon_sandbox_contract::LeaseId;
use trogon_std::env::InMemoryEnv;

use crate::agent::tests::{agent_router, serve, serve_unversioned, target};
use crate::agent::{ActorAgent, AgentError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn identity_router() -> Result<Router, Box<dyn std::error::Error>> {
    let identity = ActorIdentity {
        actor_id: ActorId::new("actor-1")?,
    };
    Ok(Router::new().route(
        IDENTITY_PATH,
        get(move || {
            let identity = identity.clone();
            async move { axum::Json(identity) }
        }),
    ))
}

fn bind_request() -> BindLeaseRequest {
    BindLeaseRequest {
        lease_id: LeaseId::new("01J0"),
        entropy_seed: None,
        env: Default::default(),
        working_dir: None,
        exposed_port: None,
    }
}

#[tokio::test]
async fn every_call_carries_the_workers_protocol_version() -> TestResult {
    let expected = GUEST_PROTOCOL_VERSION.to_string();
    let identity = ActorIdentity {
        actor_id: ActorId::new("actor-1")?,
    };
    let agent = serve(
        target()?,
        Router::new()
            .route(
                IDENTITY_PATH,
                get({
                    let expected = expected.clone();
                    move |headers: HeaderMap| {
                        let identity = identity.clone();
                        let expected = expected.clone();
                        async move {
                            assert_eq!(headers[WORKER_PROTOCOL_VERSION_HEADER], expected.as_str());
                            axum::Json(identity)
                        }
                    }
                }),
            )
            .route(
                BIND_PATH,
                post(move |headers: HeaderMap| {
                    let expected = expected.clone();
                    async move {
                        assert_eq!(headers[WORKER_PROTOCOL_VERSION_HEADER], expected.as_str());
                        StatusCode::NO_CONTENT
                    }
                }),
            ),
    )
    .await?;

    assert_eq!(agent.identity().await?.actor_id.as_str(), "actor-1");
    agent.bind(&bind_request()).await?;
    Ok(())
}

#[tokio::test]
async fn an_agent_that_sends_no_version_is_refused_by_default() -> TestResult {
    let agent = serve_unversioned(target()?, identity_router()?).await?;

    let error = agent.identity().await.err().ok_or("the call should be refused")?;
    assert!(
        matches!(
            &error,
            AgentError::ProtocolMismatch { reason, .. } if *reason == GuestProtocolError::VersionTooOld {
                peer_version: 0,
                min_supported: MIN_SUPPORTED_AGENT_PROTOCOL_VERSION,
                this_version: GUEST_PROTOCOL_VERSION,
            }
        ),
        "got {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_agent_with_a_garbled_version_is_refused() -> TestResult {
    let agent = serve_unversioned(
        target()?,
        identity_router()?.layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async move {
                response.headers_mut().insert(
                    trogon_sandbox_actor_agent::AGENT_PROTOCOL_VERSION_HEADER,
                    axum::http::HeaderValue::from_static("not-a-number"),
                );
                response
            },
        )),
    )
    .await?;

    let error = agent.identity().await.err().ok_or("the call should be refused")?;
    assert!(
        matches!(
            &error,
            AgentError::ProtocolMismatch { reason, .. } if *reason == GuestProtocolError::VersionMalformed
        ),
        "got {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn the_override_admits_an_agent_that_predates_the_handshake() -> TestResult {
    let agent = serve_unversioned(target()?, identity_router()?)
        .await?
        .with_min_agent_protocol_version(0);

    assert_eq!(agent.identity().await?.actor_id.as_str(), "actor-1");
    Ok(())
}

#[tokio::test]
async fn the_environment_lowers_the_minimum_for_a_rollout() -> TestResult {
    let agent = serve_unversioned(target()?, identity_router()?).await?;
    let env = InMemoryEnv::new();
    env.set(MIN_AGENT_PROTOCOL_VERSION_ENV, "0");
    let lenient = ActorAgent::new(agent_router(&agent), target()?, &env)?;

    assert_eq!(lenient.identity().await?.actor_id.as_str(), "actor-1");
    Ok(())
}

#[tokio::test]
async fn an_agent_below_a_raised_minimum_is_refused() -> TestResult {
    let agent = serve(target()?, identity_router()?)
        .await?
        .with_min_agent_protocol_version(GUEST_PROTOCOL_VERSION + 1);

    let error = agent.identity().await.err().ok_or("the call should be refused")?;
    assert!(
        matches!(
            &error,
            AgentError::ProtocolMismatch { reason, .. } if *reason == GuestProtocolError::VersionTooOld {
                peer_version: GUEST_PROTOCOL_VERSION,
                min_supported: GUEST_PROTOCOL_VERSION + 1,
                this_version: GUEST_PROTOCOL_VERSION,
            }
        ),
        "got {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_typed_426_from_the_agent_is_a_protocol_mismatch_not_a_plain_http_error() -> TestResult {
    let refusal = GuestProtocolError::VersionTooOld {
        peer_version: 1,
        min_supported: 2,
        this_version: 2,
    };
    let body = refusal.clone();
    let agent = serve(
        target()?,
        Router::new().route(
            BIND_PATH,
            post(move || {
                let body = body.clone();
                async move { (StatusCode::UPGRADE_REQUIRED, axum::Json(body)) }
            }),
        ),
    )
    .await?;

    let error = agent
        .bind(&bind_request())
        .await
        .err()
        .ok_or("the bind should be refused")?;
    assert!(
        matches!(&error, AgentError::ProtocolMismatch { reason, .. } if *reason == refusal),
        "got {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_untyped_426_stays_a_plain_http_error() -> TestResult {
    let agent = serve(
        target()?,
        Router::new().route(
            BIND_PATH,
            post(|| async { (StatusCode::UPGRADE_REQUIRED, "upgrade please") }),
        ),
    )
    .await?;

    let error = agent
        .bind(&bind_request())
        .await
        .err()
        .ok_or("the bind should be refused")?;
    assert!(matches!(error, AgentError::Http { status: 426, .. }), "got {error:?}");
    Ok(())
}

#[tokio::test]
async fn readiness_gives_up_at_once_on_a_protocol_mismatch() -> TestResult {
    let agent = serve_unversioned(
        target()?,
        Router::new().route(READYZ_PATH, get(|| async { StatusCode::OK })),
    )
    .await?;

    let started = tokio::time::Instant::now();
    let error = agent
        .wait_ready(std::time::Duration::from_secs(20))
        .await
        .err()
        .ok_or("readiness should be refused")?;
    assert!(matches!(error, AgentError::ProtocolMismatch { .. }), "{error:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    Ok(())
}

#[tokio::test]
async fn an_unreadable_identity_is_a_malformed_body() -> TestResult {
    let agent = serve(
        target()?,
        Router::new().route(IDENTITY_PATH, get(|| async { "not json" })),
    )
    .await?;

    let error = agent.identity().await.err().ok_or("the body should not parse")?;
    assert!(matches!(error, AgentError::Malformed { .. }), "{error:?}");
    Ok(())
}

#[test]
fn an_unparseable_minimum_override_is_refused() -> TestResult {
    let env = InMemoryEnv::new();
    env.set(MIN_AGENT_PROTOCOL_VERSION_ENV, "not-a-number");
    let error = ActorAgent::new("127.0.0.1:1".parse()?, target()?, &env)
        .err()
        .ok_or("an unreadable override must be refused")?;
    assert_eq!(error.name, MIN_AGENT_PROTOCOL_VERSION_ENV);
    Ok(())
}
