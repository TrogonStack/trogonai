use std::sync::Mutex;

use async_nats::{Message, Subject};
use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseRequest, ProviderKind, ProviderLease, TerminalDimension};
use trogon_sandbox_protocol::{ProviderError, SnapshotError};
use trogon_sandbox_ssh_gateway::{GuestChannel, GuestError, GuestShell};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Default)]
struct Recorder {
    cancelled: Mutex<Vec<LeaseId>>,
}

#[async_trait]
impl Provider for Recorder {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Daytona
    }

    async fn lease(&self, _req: &LeaseRequest, _lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        Err(ProviderError::Snapshot(SnapshotError::Unsupported))
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        self.cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(lease_id.clone());
        Ok(())
    }
}

/// A guest shell that reaches nothing and remembers which leases were released.
#[derive(Default)]
struct Released {
    leases: Mutex<Vec<LeaseId>>,
}

#[async_trait]
impl GuestShell for Released {
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
        Err(GuestError::Unavailable(Box::new(std::io::Error::other("no guest"))))
    }

    async fn release(&self, lease_id: &LeaseId) {
        self.leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(lease_id.clone());
    }

    fn not_held_reason(&self) -> &'static str {
        "no guest"
    }
}

/// Terminals over `guest`, on a client that never connects: nothing in these tests publishes.
async fn terminals(guest: Arc<Released>) -> Result<Arc<DaytonaTerminals>, Box<dyn std::error::Error + Send + Sync>> {
    let client = async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .connect("nats://127.0.0.1:1")
        .await?;
    Ok(Arc::new(DaytonaTerminals::new(guest, client)))
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
        cancel_lease_id("sandbox.provider.daytona.cancel.lease-1"),
        Some(LeaseId::new("lease-1"))
    );
    assert_eq!(cancel_lease_id("lease-1"), Some(LeaseId::new("lease-1")));
}

#[test]
fn an_empty_trailing_token_names_no_lease() {
    assert_eq!(cancel_lease_id("sandbox.provider.daytona.cancel."), None);
}

#[tokio::test]
async fn a_cancel_without_a_lease_is_ignored() -> TestResult {
    let provider = Arc::new(Recorder::default());
    let guest = Arc::new(Released::default());
    handle_cancel_msg(
        message("sandbox.provider.daytona.cancel."),
        provider.clone(),
        ActiveLeases::default(),
        Arc::new(RedispatchGuard::new()),
        terminals(Arc::clone(&guest)).await?,
    )
    .await;
    assert!(
        guest
            .leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    assert!(
        provider
            .cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn a_cancel_stops_the_launch_and_releases_the_lease() -> TestResult {
    let provider = Arc::new(Recorder::default());
    let guest = Arc::new(Released::default());
    let active = ActiveLeases::default();
    let token = tokio_util::sync::CancellationToken::new();
    active.lock().await.insert(LeaseId::new("lease-1"), token.clone());
    handle_cancel_msg(
        message("sandbox.provider.daytona.cancel.lease-1"),
        provider.clone(),
        active,
        Arc::new(RedispatchGuard::new()),
        terminals(Arc::clone(&guest)).await?,
    )
    .await;
    assert!(token.is_cancelled());
    let released = guest
        .leases
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(released, vec![LeaseId::new("lease-1")]);
    let cancelled = provider
        .cancelled
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(cancelled, vec![LeaseId::new("lease-1")]);
    Ok(())
}
