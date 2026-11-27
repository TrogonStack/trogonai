use std::sync::Mutex;

use async_nats::{Message, Subject};
use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseRequest, ProviderKind, ProviderLease};
use trogon_sandbox_protocol::{ProviderError, SnapshotError};

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
    handle_cancel_msg(
        message("sandbox.provider.daytona.cancel."),
        provider.clone(),
        ActiveLeases::default(),
        Arc::new(RedispatchGuard::new()),
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

#[tokio::test]
async fn a_cancel_stops_the_launch_and_releases_the_lease() -> TestResult {
    let provider = Arc::new(Recorder::default());
    let active = ActiveLeases::default();
    let token = tokio_util::sync::CancellationToken::new();
    active.lock().await.insert(LeaseId::new("lease-1"), token.clone());
    handle_cancel_msg(
        message("sandbox.provider.daytona.cancel.lease-1"),
        provider.clone(),
        active,
        Arc::new(RedispatchGuard::new()),
    )
    .await;
    assert!(token.is_cancelled());
    let cancelled = provider
        .cancelled
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(cancelled, vec![LeaseId::new("lease-1")]);
    Ok(())
}
