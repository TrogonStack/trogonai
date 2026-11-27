use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::{Serialize, Serializer};
use trogon_sandbox_actor_agent::{EXEC_PATH, IDENTITY_PATH, READYZ_PATH};
use trogon_sandbox_contract::ReadFileRequest;
use trogon_std::env::InMemoryEnv;

use crate::agent::tests::{
    agent_router, parse, request, serve, serve_raw_reply, serve_unversioned, target, target_for_tenant,
};
use crate::agent::{ActorAgent, ActorTarget, AgentError, DiscardChunks};
use crate::naming::{ActorName, Atespace};
use crate::router_tls::RouterTlsConfig;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Unserializable;

impl Serialize for Unserializable {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("this request cannot be encoded"))
    }
}

#[test]
fn the_target_names_its_header_and_port() -> TestResult {
    let target = target()?;
    assert_eq!(target.port(), 8110);
    assert_eq!(
        target.header_value(),
        format!(
            "tenant-a/{}",
            ActorName::for_lease(&"01J0".to_string().into())?.as_str()
        )
    );
    assert!(target.to_string().ends_with(":8110"));
    Ok(())
}

#[tokio::test]
async fn readiness_waits_for_a_bound_lease_and_gives_up_on_the_deadline() -> TestResult {
    let agent = serve(
        target()?,
        Router::new().route(READYZ_PATH, get(|| async { StatusCode::SERVICE_UNAVAILABLE })),
    )
    .await?;

    let error = agent
        .wait_ready(Duration::from_millis(50))
        .await
        .err()
        .ok_or("readiness should time out")?;
    assert!(matches!(error, AgentError::NotReady { .. }), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn an_unreachable_actor_is_distinguished_from_a_refusal() -> TestResult {
    let agent = ActorAgent::new("127.0.0.1:1".parse()?, target()?, &InMemoryEnv::new());
    let request: ReadFileRequest = parse(r#"{"lease_id":"01J0","path":"/workspace/x"}"#)?;

    let error = agent.read_file(&request).await.err().ok_or("nothing is listening")?;
    assert!(matches!(error, AgentError::Unreachable { .. }), "got {error:?}");
    Ok(())
}

#[tokio::test]
async fn a_connect_request_carries_the_routing_header_and_the_target_port() -> TestResult {
    // The fake listener in `serve` returns 421 unless the header and authority port
    // match `target()`; a mismatched target proves the tunnel refuses to be established
    // rather than proving the routing values themselves.
    let agent = serve(
        target()?,
        Router::new().route(READYZ_PATH, get(|| async { StatusCode::OK })),
    )
    .await?;
    let router = agent_router(&agent);

    let matching = ActorAgent::new(router.clone(), target()?, &InMemoryEnv::new());
    matching.wait_ready(Duration::from_secs(1)).await?;

    let other_tenant = target_for_tenant("tenant-b")?;
    let mismatched = ActorAgent::new(router.clone(), other_tenant, &InMemoryEnv::new());
    let error = mismatched
        .wait_ready(Duration::from_millis(200))
        .await
        .err()
        .ok_or("a misdirected tunnel must not be established")?;
    assert!(matches!(error, AgentError::NotReady { .. }), "{error:?}");

    let other_port = ActorTarget::new(
        Atespace::for_tenant(&trogon_sandbox_contract::TenantId::new("tenant-a")?)?,
        ActorName::for_lease(&trogon_sandbox_contract::LeaseId::new("01J0"))?,
        9999,
    );
    let wrong_port = ActorAgent::new(router, other_port, &InMemoryEnv::new());
    let error = wrong_port
        .wait_ready(Duration::from_millis(200))
        .await
        .err()
        .ok_or("a different port must not be established")?;
    assert!(matches!(error, AgentError::NotReady { .. }), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn a_router_that_hangs_up_before_answering_connect_is_unreachable() -> TestResult {
    let router = serve_raw_reply(b"").await?;
    let agent = ActorAgent::new(router, target()?, &InMemoryEnv::new());

    let error = agent.identity().await.err().ok_or("the router hung up")?;
    assert!(
        matches!(&error, AgentError::Unreachable { message, .. } if message.contains("closed the connection")),
        "got {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_router_that_refuses_connect_is_unreachable_with_its_status() -> TestResult {
    let router = serve_raw_reply(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await?;
    let agent = ActorAgent::new(router, target()?, &InMemoryEnv::new());

    let error = agent.identity().await.err().ok_or("the router refused")?;
    assert!(
        matches!(&error, AgentError::Unreachable { message, .. } if message.contains("502")),
        "got {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_router_that_answers_gibberish_is_unreachable() -> TestResult {
    let router = serve_raw_reply(b"gibberish\r\n\r\n").await?;
    let agent = ActorAgent::new(router, target()?, &InMemoryEnv::new());

    let error = agent.identity().await.err().ok_or("the router answered gibberish")?;
    assert!(
        matches!(&error, AgentError::Unreachable { message, .. } if message.contains("unreadable")),
        "got {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_control_call_that_outlives_its_deadline_is_unreachable() -> TestResult {
    let agent = serve_unversioned(
        target()?,
        Router::new().route(
            IDENTITY_PATH,
            get(|| async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                StatusCode::OK
            }),
        ),
    )
    .await?
    .with_transport_timeouts(Duration::from_millis(50), Duration::ZERO);

    let error = agent.identity().await.err().ok_or("the agent never answered")?;
    assert!(
        matches!(&error, AgentError::Unreachable { message, .. } if message.contains("timed out")),
        "got {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_exec_gets_its_own_timeout_plus_the_transport_margin() -> TestResult {
    let agent = serve_unversioned(
        target()?,
        Router::new().route(
            EXEC_PATH,
            post(|| async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                StatusCode::OK
            }),
        ),
    )
    .await?
    .with_transport_timeouts(Duration::from_secs(60), Duration::from_millis(50));
    let mut run = request()?;
    run.timeout_seconds = 0;

    let error = agent
        .exec(&run, &DiscardChunks)
        .await
        .err()
        .ok_or("the router never answered")?;
    assert!(
        matches!(&error, AgentError::Unreachable { message, .. } if message.contains("timed out")),
        "got {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_request_that_cannot_be_encoded_is_never_sent() -> TestResult {
    let agent = serve(target()?, Router::new()).await?;

    let error = agent
        .call::<_, serde_json::Value>("/never", &Unserializable)
        .await
        .err()
        .ok_or("nothing can be encoded")?;
    assert!(matches!(error, AgentError::Request { .. }), "got {error:?}");
    Ok(())
}

#[test]
fn a_tunnel_that_ended_cleanly_logs_nothing_and_a_broken_one_is_recorded() {
    crate::agent::actor_agent::log_tunnel_end("actor", Ok::<(), String>(()));
    crate::agent::actor_agent::log_tunnel_end("actor", Err("connection reset".to_string()));
}

#[tokio::test]
async fn a_router_tls_identity_that_cannot_be_loaded_makes_the_actor_unreachable() -> TestResult {
    let tls = RouterTlsConfig::new(
        "/nonexistent/ca.pem",
        "/nonexistent/client.pem",
        "/nonexistent/client.key",
        "localhost",
    )?;
    let agent = ActorAgent::new("127.0.0.1:1".parse()?, target()?, &InMemoryEnv::new()).with_router_tls(tls);
    let request: ReadFileRequest = parse(r#"{"lease_id":"01J0","path":"/workspace/x"}"#)?;

    let error = agent
        .read_file(&request)
        .await
        .err()
        .ok_or("the identity does not exist")?;
    assert!(matches!(error, AgentError::Unreachable { .. }), "got {error:?}");
    Ok(())
}
