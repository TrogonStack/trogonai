mod fixture;

use std::error::Error;
use std::time::Duration;

use async_nats::{Client, Message, Request, RequestError, RequestErrorKind};
use bytes::Bytes;
use clap::Parser;
use futures::future::join_all;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{LeaseError, LeaseId, LeaseResponse, UpdateLeaseTagsResponse, WriteFilesResponse};
use trogon_sandbox_protocol::nats_transfer;
use trogon_sandbox_protocol::subjects::{self, LEASE_REQUEST, SNAPSHOT_ACCOUNTING_REQUEST};
use trogon_std::env::InMemoryEnv;

use self::fixture::{Fixture, GARBAGE, Row};
use super::{ServeError, run};
use crate::cli::Cli;

type TestResult = Result<(), Box<dyn Error>>;

/// How long a route that answers may take, including a provider dispatch that finds no worker.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a route that must stay quiet is watched.
const SILENCE: Duration = Duration::from_millis(500);
const READY_ATTEMPTS: usize = 120;
const READY_TRY: Duration = Duration::from_millis(500);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(60);

fn cli(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("trogon-sandbox-broker").chain(args.iter().copied()))
}

/// A broker on `nats_url` that binds an ephemeral metrics port, keeps one replica of each store,
/// and gives up quickly on providers and registries that are not there.
fn broker_cli(nats_url: &str, extra: &[&str]) -> Result<Cli, clap::Error> {
    let mut args = vec![
        "--nats-url",
        nats_url,
        "--metrics-addr",
        "127.0.0.1:0",
        "--jetstream-replicas",
        "1",
        "--provider-dispatch-timeout-secs",
        "1",
        "--snapshot-registry",
        "127.0.0.1:1",
    ];
    args.extend_from_slice(extra);
    cli(&args)
}

#[tokio::test]
async fn a_nats_url_that_does_not_parse_is_refused() -> TestResult {
    let cli = cli(&[
        "--nats-url",
        "not a url",
        "--metrics-addr",
        "127.0.0.1:0",
        "--insecure-dev",
    ])?;

    let outcome = run(cli, &InMemoryEnv::new(), CancellationToken::new()).await;

    assert!(matches!(outcome, Err(ServeError::Connect(_))), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn enforced_auth_without_a_permission_service_is_refused() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let url = format!("nats://{}", server.address());
    let cli = broker_cli(
        &url,
        &["--quota-backend", "in-memory", "--idempotency-backend", "in-memory"],
    )?;

    let outcome = run(cli, &InMemoryEnv::new(), CancellationToken::new()).await;

    assert!(matches!(outcome, Err(ServeError::AuthSetup(_))), "{outcome:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_route_is_served_until_shutdown() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let url = format!("nats://{}", server.address());
    let env = InMemoryEnv::new();
    let shutdown = CancellationToken::new();
    let fixture = Fixture::new()?;

    let broker = run(broker_cli(&url, &["--insecure-dev"])?, &env, shutdown.clone());
    tokio::pin!(broker);
    tokio::select! {
        stopped = &mut broker => return Err(format!("the broker stopped before shutdown: {stopped:?}").into()),
        exercised = exercise(&client, &fixture) => exercised?,
    }
    shutdown.cancel();

    tokio::time::timeout(SHUTDOWN_TIMEOUT, broker).await??;
    Ok(())
}

async fn exercise(client: &Client, fixture: &Fixture) -> TestResult {
    wait_until_serving(client).await?;

    let mut rows = fixture.tenant_rows()?;
    rows.extend(fixture.lease_rows()?);
    rows.extend(fixture.event_rows()?);
    rows.extend(fixture.decode_rows());
    assert_eq!(unanswered(client, &rows).await, Vec::<String>::new());
    assert_decode_failures_are_refused(client, fixture).await?;
    assert_transfers_are_answered(client, fixture).await?;
    assert_eq!(answered(client, &fixture.silent_rows()?).await, Vec::<String>::new());

    for (subject, payload) in fixture.published_rows()? {
        client.publish(subject, payload).await?;
    }
    client.flush().await?;
    wait_until_serving(client).await
}

/// Polls the snapshot accounting route, the last request route to subscribe, until it answers.
async fn wait_until_serving(client: &Client) -> TestResult {
    for _ in 0..READY_ATTEMPTS {
        if ask(
            client,
            SNAPSHOT_ACCOUNTING_REQUEST,
            Bytes::from_static(GARBAGE),
            READY_TRY,
        )
        .await
        .is_ok()
        {
            return Ok(());
        }
        tokio::time::sleep(READY_TRY).await;
    }
    Err("the broker never answered the snapshot accounting route".into())
}

async fn ask(client: &Client, subject: &str, payload: Bytes, timeout: Duration) -> Result<Message, RequestError> {
    client
        .send_request(
            subject.to_owned(),
            Request::new().payload(payload).timeout(Some(timeout)),
        )
        .await
}

/// Sends every row at once and lists the subjects that did not answer, with the reason.
async fn unanswered(client: &Client, rows: &[Row]) -> Vec<String> {
    let replies = join_all(
        rows.iter()
            .map(|(subject, payload)| ask(client, subject, payload.clone(), REPLY_TIMEOUT)),
    )
    .await;
    rows.iter()
        .zip(replies)
        .filter_map(|((subject, _), reply)| reply.err().map(|error| format!("{subject}: {error}")))
        .collect()
}

/// Sends every row at once and lists the subjects that answered or had no subscriber.
async fn answered(client: &Client, rows: &[Row]) -> Vec<String> {
    let replies = join_all(
        rows.iter()
            .map(|(subject, payload)| ask(client, subject, payload.clone(), SILENCE)),
    )
    .await;
    rows.iter()
        .zip(replies)
        .filter_map(|((subject, _), reply)| match reply {
            Err(error) if matches!(error.kind(), RequestErrorKind::TimedOut) => None,
            Err(error) => Some(format!("{subject}: {error}")),
            Ok(_) => Some(format!("{subject}: answered")),
        })
        .collect()
}

async fn assert_decode_failures_are_refused(client: &Client, fixture: &Fixture) -> TestResult {
    let lease = ask(client, LEASE_REQUEST, Bytes::from_static(GARBAGE), REPLY_TIMEOUT).await?;
    let response: LeaseResponse = serde_json::from_slice(&lease.payload)?;
    assert!(
        matches!(&response, LeaseResponse::Denied(LeaseError::Backend { message }) if message.starts_with("decode error")),
        "{response:?}"
    );

    let subject = subjects::lease_tags_update(fixture.lease());
    let tags = ask(client, &subject, Bytes::from_static(GARBAGE), REPLY_TIMEOUT).await?;
    let response: UpdateLeaseTagsResponse = serde_json::from_slice(&tags.payload)?;
    assert!(matches!(response, UpdateLeaseTagsResponse::Denied(_)), "{response:?}");
    Ok(())
}

async fn assert_transfers_are_answered(client: &Client, fixture: &Fixture) -> TestResult {
    for (subject, payload) in fixture.transfer_rows()? {
        nats_transfer::request(client, &subject, payload, None, REPLY_TIMEOUT)
            .await
            .map_err(|error| format!("{subject}: {error}"))?;
    }

    let foreign = subjects::lease_write_files(&LeaseId::new("lease-2"));
    let reply = nats_transfer::request(client, &foreign, fixture.write_files()?, None, REPLY_TIMEOUT).await?;
    let response: WriteFilesResponse = serde_json::from_slice(&reply)?;
    assert!(matches!(response, WriteFilesResponse::Denied(_)), "{response:?}");
    Ok(())
}
