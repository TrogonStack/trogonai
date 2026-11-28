use std::convert::Infallible;
use std::sync::Mutex;

use async_nats::{Message, Subject};
use async_trait::async_trait;
use futures::FutureExt;
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_conformance::{CountingProvider, ensure_launches_once, ensure_relaunches_after_cancel};
use trogon_sandbox_contract::TerminalDimension;
use trogon_sandbox_contract::{LeaseRequest, ProviderKind, ProviderLease};
use trogon_sandbox_protocol::{ProviderError, SnapshotError};
use trogon_sandbox_ssh_gateway::{GuestChannel, GuestError, GuestShell};

use crate::E2bWorkerError;
use crate::terminal::E2bTerminals;
use crate::worker::lease_dispatch::handle_lease_msg;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct Recorder {
    cancelled: Mutex<Vec<LeaseId>>,
    fail: bool,
}

#[async_trait]
impl Provider for Recorder {
    fn kind(&self) -> ProviderKind {
        ProviderKind::E2b
    }

    async fn lease(&self, _req: &LeaseRequest, _lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        Err(ProviderError::Snapshot(SnapshotError::Unsupported))
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        self.cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(lease_id.clone());
        if self.fail {
            return Err(ProviderError::backend("e2b unavailable"));
        }
        Ok(())
    }
}

/// A terminal hub for a worker that holds no sandbox.
fn idle_terminals(client: &async_nats::Client) -> Arc<E2bTerminals> {
    Arc::new(E2bTerminals::new(Arc::new(NoGuest), client.clone()))
}

struct NoGuest;

#[async_trait]
impl GuestShell for NoGuest {
    async fn holds(&self, _lease_id: &LeaseId) -> bool {
        false
    }

    async fn open(
        &self,
        _lease_id: &LeaseId,
        _size: (TerminalDimension, TerminalDimension),
        _term: &str,
        _command: Option<String>,
    ) -> Result<GuestChannel, GuestError> {
        Err(GuestError::NoExitStatus)
    }

    async fn release(&self, _lease_id: &LeaseId) {}
}

fn message(subject: &str) -> Message {
    Message {
        subject: Subject::from(subject),
        reply: None,
        payload: Default::default(),
        headers: None,
        status: None,
        description: None,
        length: 0,
    }
}

#[test]
fn the_lease_is_the_last_subject_token() {
    assert_eq!(
        cancel_lease_id("sandbox.provider.e2b.cancel.lease-1"),
        Some(LeaseId::new("lease-1"))
    );
    assert_eq!(cancel_lease_id("lease-1"), Some(LeaseId::new("lease-1")));
}

#[test]
fn an_empty_trailing_token_names_no_lease() {
    assert_eq!(cancel_lease_id("sandbox.provider.e2b.cancel."), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_releases_the_lease_and_stops_its_launch() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let provider = Arc::new(Recorder::default());
    let active = ActiveLeases::default();
    let launching = CancellationToken::new();
    active.lock().await.insert(LeaseId::new("lease-1"), launching.clone());
    handle_cancel_msg(
        message("sandbox.provider.e2b.cancel.lease-1"),
        provider.clone(),
        active,
        Arc::new(RedispatchGuard::new()),
        idle_terminals(&client),
    )
    .await;
    assert!(launching.is_cancelled());
    let cancelled = provider
        .cancelled
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(cancelled, vec![LeaseId::new("lease-1")]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_without_a_lease_is_ignored() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let provider = Arc::new(Recorder::default());
    handle_cancel_msg(
        message("sandbox.provider.e2b.cancel."),
        provider.clone(),
        ActiveLeases::default(),
        Arc::new(RedispatchGuard::new()),
        idle_terminals(&client),
    )
    .await;
    assert!(
        provider
            .cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_the_provider_refuses_is_still_handled() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let provider = Arc::new(Recorder {
        fail: true,
        ..Default::default()
    });
    handle_cancel_msg(
        message("sandbox.provider.e2b.cancel.lease-1"),
        provider.clone(),
        ActiveLeases::default(),
        Arc::new(RedispatchGuard::new()),
        idle_terminals(&client),
    )
    .await;
    assert_eq!(provider.cancelled.lock().map(|held| held.len()).unwrap_or_default(), 1);
    Ok(())
}

/// The worker's own lease handler over `provider`, as the redispatch checks drive it.
fn lease_handler(
    client: &async_nats::Client,
    provider: Arc<dyn Provider>,
    active: ActiveLeases,
    grants: Arc<RedispatchGuard>,
) -> impl Fn(Message) -> BoxFuture<'static, Result<(), E2bWorkerError>> {
    let client = client.clone();
    let js = async_nats::jetstream::new(client.clone());
    move |msg| {
        handle_lease_msg(
            msg,
            client.clone(),
            js.clone(),
            Arc::clone(&provider),
            Arc::clone(&active),
            Arc::clone(&grants),
        )
        .boxed()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redispatched_lease_launches_once() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let counting = CountingProvider::new(ProviderKind::E2b);
    let handle = lease_handler(
        &client,
        counting.clone(),
        ActiveLeases::default(),
        Arc::new(RedispatchGuard::new()),
    );

    ensure_launches_once(&client, &counting, &LeaseId::new("redispatch-lease"), handle).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_lease_id_launches_afresh_when_dispatched_again() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let counting = CountingProvider::new(ProviderKind::E2b);
    let provider: Arc<dyn Provider> = counting.clone();
    let active = ActiveLeases::default();
    let grants = Arc::new(RedispatchGuard::new());
    let terminals = idle_terminals(&client);
    let handle = lease_handler(&client, Arc::clone(&provider), Arc::clone(&active), Arc::clone(&grants));
    let cancel = move |msg| {
        handle_cancel_msg(
            msg,
            Arc::clone(&provider),
            Arc::clone(&active),
            Arc::clone(&grants),
            Arc::clone(&terminals),
        )
        .map(Ok::<(), Infallible>)
    };

    ensure_relaunches_after_cancel(&client, &counting, &LeaseId::new("redispatch-cancel"), handle, cancel).await?;
    Ok(())
}
