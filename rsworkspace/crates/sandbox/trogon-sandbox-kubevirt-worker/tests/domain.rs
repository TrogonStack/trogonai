//! get_domain for KubeVirt leases: the worker holding the lease says VMIs publish no ports, every other worker stays
//! silent so the holder can answer.

mod guest_support;

use std::sync::Arc;
use std::time::Duration;

use async_nats::{Client, HeaderMap, Message, SubscribeError};
use async_trait::async_trait;
use futures::StreamExt;
use guest_support::{Nats, TestResult, request_until_answered, unanswered};
use tokio::task::JoinHandle;
use trogon_sandbox_contract::{GetSandboxDomainRequest, GetSandboxDomainResponse, LeaseId, ProviderKind};
use trogon_sandbox_kubevirt_worker::{LeaseHolding, ServeDomainError, run_get_domain_handler, serve_domain};
use trogon_sandbox_protocol::NatsTransferError;
use trogon_sandbox_protocol::subjects::provider_get_domain;

/// Holds `lease-1` only.
struct Holder;

#[async_trait]
impl LeaseHolding for Holder {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        lease_id.as_str() == "lease-1"
    }
}

fn holder() -> Arc<dyn LeaseHolding> {
    Arc::new(Holder)
}

fn subject(lease: &str) -> String {
    provider_get_domain(ProviderKind::KubeVirt, &LeaseId::new(lease))
}

fn body(lease: &str) -> TestResult<Vec<u8>> {
    let request = GetSandboxDomainRequest {
        lease_id: LeaseId::new(lease),
        port: 8080,
        wait_ms: None,
    };
    Ok(serde_json::to_vec(&request)?)
}

/// A delivered get_domain request for `lease`, as the handler would receive it.
fn message(lease: &str, reply: Option<&str>, payload: Vec<u8>, headers: Option<HeaderMap>) -> Message {
    Message {
        subject: subject(lease).into(),
        reply: reply.map(Into::into),
        length: payload.len(),
        payload: payload.into(),
        headers,
        status: None,
        description: None,
    }
}

/// A transfer header naming a version no peer speaks.
fn unknown_transfer() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("Sandbox-Transfer", "chunks-v0");
    headers
}

struct Rig {
    _nats: Nats,
    client: Client,
    handler: JoinHandle<Result<(), SubscribeError>>,
}

async fn rig() -> TestResult<Rig> {
    let nats = Nats::start().await;
    let client = nats.client().await?;
    let handler = tokio::spawn(run_get_domain_handler(client.clone(), holder()));
    Ok(Rig {
        _nats: nats,
        client,
        handler,
    })
}

/// Asks for `lease-1`'s domain, retrying until the handler, which subscribes in a spawned task, answers.
async fn ask(client: &Client) -> TestResult<GetSandboxDomainResponse> {
    let reply = request_until_answered(client, &subject("lease-1"), body("lease-1")?).await?;
    Ok(serde_json::from_slice(&reply.payload)?)
}

#[tokio::test]
async fn a_held_lease_is_told_its_vm_publishes_no_ports() -> TestResult {
    let rig = rig().await?;

    assert_eq!(ask(&rig.client).await?, GetSandboxDomainResponse::Unsupported);
    Ok(())
}

#[tokio::test]
async fn a_lease_the_worker_does_not_hold_gets_no_answer() -> TestResult {
    let rig = rig().await?;
    ask(&rig.client).await?;

    assert!(unanswered(&rig.client, &subject("lease-other"), body("lease-other")?).await);
    Ok(())
}

#[tokio::test]
async fn a_request_that_cannot_be_answered_does_not_stop_the_handler() -> TestResult {
    let rig = rig().await?;
    ask(&rig.client).await?;

    assert!(unanswered(&rig.client, &subject("lease-1"), b"not json".to_vec()).await);

    assert_eq!(ask(&rig.client).await?, GetSandboxDomainResponse::Unsupported);
    Ok(())
}

#[tokio::test]
async fn the_handler_returns_once_the_client_drains() -> TestResult {
    let rig = rig().await?;
    ask(&rig.client).await?;

    rig.client.drain().await?;

    tokio::time::timeout(Duration::from_secs(10), rig.handler).await???;
    Ok(())
}

#[tokio::test]
async fn serving_a_held_lease_replies_unsupported() -> TestResult {
    let nats = Nats::start().await;
    let client = nats.client().await?;
    let inbox = client.new_inbox();
    let mut replies = client.subscribe(inbox.clone()).await?;

    serve_domain(
        client.clone(),
        holder(),
        message("lease-1", Some(&inbox), body("lease-1")?, None),
    )
    .await?;

    let reply = tokio::time::timeout(Duration::from_secs(10), replies.next())
        .await?
        .ok_or("no reply")?;
    let response: GetSandboxDomainResponse = serde_json::from_slice(&reply.payload)?;
    assert_eq!(response, GetSandboxDomainResponse::Unsupported);
    Ok(())
}

#[tokio::test]
async fn serving_an_unheld_lease_or_a_request_without_a_reply_sends_nothing() -> TestResult {
    let nats = Nats::start().await;
    let client = nats.client().await?;
    let inbox = client.new_inbox();
    let mut replies = client.subscribe(inbox.clone()).await?;

    serve_domain(
        client.clone(),
        holder(),
        message("lease-other", Some(&inbox), body("lease-other")?, None),
    )
    .await?;
    serve_domain(
        client.clone(),
        holder(),
        message("lease-1", None, body("lease-1")?, None),
    )
    .await?;
    client.flush().await?;

    let reply = tokio::time::timeout(Duration::from_millis(400), replies.next()).await;
    assert!(reply.is_err(), "nothing must be published");
    Ok(())
}

#[tokio::test]
async fn serving_a_request_that_does_not_parse_fails() -> TestResult {
    let nats = Nats::start().await;
    let client = nats.client().await?;

    let served = serve_domain(
        client,
        holder(),
        message("lease-1", Some("_INBOX.x"), b"not json".to_vec(), None),
    )
    .await;

    assert!(matches!(served, Err(ServeDomainError::Json(_))));
    Ok(())
}

#[tokio::test]
async fn serving_a_request_in_an_unknown_transfer_version_fails() -> TestResult {
    let nats = Nats::start().await;
    let client = nats.client().await?;
    let request = message("lease-1", Some("_INBOX.x"), body("lease-1")?, Some(unknown_transfer()));

    let served = serve_domain(client, holder(), request).await;

    assert!(matches!(
        served,
        Err(ServeDomainError::Transfer(NatsTransferError::UnknownVersion))
    ));
    Ok(())
}

#[test]
fn every_serve_domain_error_describes_itself() -> TestResult {
    let json = serde_json::from_slice::<GetSandboxDomainRequest>(b"{")
        .err()
        .ok_or("parsed")?;
    let errors = [
        ServeDomainError::Transfer(NatsTransferError::UnknownVersion),
        ServeDomainError::Json(json),
    ];
    for error in errors {
        assert!(!error.to_string().is_empty());
    }
    Ok(())
}
