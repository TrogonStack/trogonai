use clap::Parser;
use futures::StreamExt;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_protocol::subjects;
use trogon_std::env::InMemoryEnv;

use super::super::{ServeError, WorkerArgs, WorkerError, run, serve};
use super::support::{TestResult, WAIT, provider_holding};
use axum::Router;

const IMAGE: &str = "registry.example/agent@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn args(nats_url: &str, ate_endpoint: &str, extra: &[&str]) -> Result<WorkerArgs, clap::Error> {
    let mut flags = vec![
        "worker",
        "--allow-insecure-router",
        "--nats-url",
        nats_url,
        "--ate-api-endpoint",
        ate_endpoint,
        "--agent-image",
        IMAGE,
    ];
    flags.extend_from_slice(extra);
    WorkerArgs::try_parse_from(flags)
}

/// An address nothing listens on.
async fn closed_address() -> Result<String, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    Ok(listener.local_addr()?.to_string())
}

#[tokio::test]
async fn invalid_arguments_stop_before_anything_connects() -> TestResult {
    let args = args("nats://unused:4222", "http://unused:8080", &["--size-class", "bad"])?;

    let error = serve(&args, &InMemoryEnv::new(), CancellationToken::new())
        .await
        .err()
        .ok_or("served with bad arguments")?;

    assert!(matches!(error, ServeError::SizeClass { .. }));
    Ok(())
}

#[tokio::test]
async fn an_unreachable_nats_server_is_reported_with_its_url() -> TestResult {
    let nats = format!("nats://{}", closed_address().await?);
    let args = args(&nats, "http://unused:8080", &[])?;

    let error = serve(&args, &InMemoryEnv::new(), CancellationToken::new())
        .await
        .err()
        .ok_or("served without NATS")?;

    assert!(matches!(error, ServeError::Connect { ref url, .. } if url == &nats));
    Ok(())
}

#[tokio::test]
async fn an_unreachable_control_plane_is_reported_with_its_endpoint() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let nats = format!("nats://{}", server.address());
    let endpoint = format!("http://{}", closed_address().await?);
    let args = args(&nats, &endpoint, &[])?;

    let error = serve(&args, &InMemoryEnv::new(), CancellationToken::new())
        .await
        .err()
        .ok_or("served without a control plane")?;

    assert!(matches!(error, ServeError::Control { endpoint: ref reported, .. } if reported == &endpoint));
    Ok(())
}

#[tokio::test]
async fn the_worker_announces_itself_and_stops_when_cancelled() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let nats = format!("nats://{}", server.address());
    // The control plane connection is established lazily per call, so a listener that never
    // speaks is enough for startup: an empty index means recovery never needs it.
    let control = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("http://{}", control.local_addr()?);
    let holding = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = control.accept().await {
            held.push(socket);
        }
    });
    let worker_id = WorkerId::new("worker-served")?;
    let args = args(&nats, &endpoint, &["--worker-id", worker_id.as_str()])?;
    let mut beats = client
        .subscribe(subjects::worker_heartbeat(ProviderKind::Substrate, &worker_id))
        .await?;
    client.flush().await?;
    let cancel = CancellationToken::new();
    let env = InMemoryEnv::new();
    let stopper = async {
        let beat = tokio::time::timeout(WAIT, beats.next()).await;
        cancel.cancel();
        beat
    };

    let (served, beat) = tokio::time::timeout(WAIT, async {
        tokio::join!(serve(&args, &env, cancel.clone()), stopper)
    })
    .await?;

    beat?.ok_or("no heartbeat")?;
    served?;
    holding.abort();
    Ok(())
}

#[tokio::test]
async fn an_invalid_authority_mode_stops_the_worker_before_it_subscribes() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = server.jetstream().await;
    let (provider, _control, _index) = provider_holding(Router::new(), &[]).await?;
    let env = InMemoryEnv::new();
    env.set("SANDBOX_WORKER_AUTHORITY", "sometimes");

    let result = run(
        client,
        js,
        provider.clone(),
        provider.worker_id().clone(),
        CancellationToken::new(),
        &env,
    )
    .await;

    assert!(matches!(result, Err(WorkerError::Authority(_))));
    Ok(())
}

#[tokio::test]
async fn a_worker_id_that_differs_from_the_provider_owner_is_refused() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = server.jetstream().await;
    let (provider, _control, _index) = provider_holding(Router::new(), &[]).await?;

    let result = run(
        client,
        js,
        provider,
        WorkerId::new("someone-else")?,
        CancellationToken::new(),
        &InMemoryEnv::new(),
    )
    .await;

    assert!(matches!(result, Err(WorkerError::WorkerIdMismatch { .. })));
    Ok(())
}

#[test]
fn recovery_outcomes_are_logged_whether_or_not_they_succeeded() {
    super::super::serve::log_recovery(&Ok(crate::recovery::RecoveryReport::default()));
    super::super::serve::log_recovery(&Err(crate::recovery::RecoveryError::Index(
        crate::actor_index::ActorIndexError::storage(std::io::Error::other("unreadable")),
    )));
}
