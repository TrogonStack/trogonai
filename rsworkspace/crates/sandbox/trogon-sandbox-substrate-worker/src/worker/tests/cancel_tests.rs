use async_nats::Message;
use axum::Router;
use bytes::Bytes;
use trogon_sandbox_contract::{CancelIntent, LeaseId};
use trogon_sandbox_protocol::RedispatchGuard;

use super::super::handle_cancel_msg;
use super::support::{HELD, MISROUTED, TestResult, provider_holding};
use crate::ate_client::AteError;

fn cancel_message(subject: &str, payload: Bytes) -> Message {
    Message {
        subject: subject.into(),
        reply: None,
        length: payload.len(),
        payload,
        headers: None,
        status: None,
        description: None,
    }
}

#[tokio::test]
async fn a_release_ends_the_lease_and_forgets_its_grant() -> TestResult {
    let (provider, control, _index) = provider_holding(Router::new(), &[HELD, MISROUTED]).await?;
    let grants = RedispatchGuard::new();

    let message = cancel_message(
        &format!("sandbox.provider.substrate.cancel.{HELD}"),
        Bytes::from(serde_json::to_vec(&CancelIntent::Release)?),
    );
    handle_cancel_msg(&message, &provider, &grants).await;

    assert!(provider.active_lease(&LeaseId::new(HELD)).await.is_none());
    assert!(provider.active_lease(&LeaseId::new(MISROUTED)).await.is_some());
    assert!(control.delete_count() <= 1);
    Ok(())
}

#[tokio::test]
async fn a_purge_ends_the_lease() -> TestResult {
    let (provider, _control, _index) = provider_holding(Router::new(), &[HELD]).await?;
    let grants = RedispatchGuard::new();

    let message = cancel_message(
        &format!("sandbox.provider.substrate.cancel.{HELD}"),
        Bytes::from(serde_json::to_vec(&CancelIntent::Purge)?),
    );
    handle_cancel_msg(&message, &provider, &grants).await;

    assert!(provider.active_lease(&LeaseId::new(HELD)).await.is_none());
    Ok(())
}

#[tokio::test]
async fn a_cancel_for_a_lease_nobody_holds_changes_nothing() -> TestResult {
    let (provider, _control, _index) = provider_holding(Router::new(), &[HELD]).await?;
    let grants = RedispatchGuard::new();

    let message = cancel_message("sandbox.provider.substrate.cancel.01J9", Bytes::new());
    handle_cancel_msg(&message, &provider, &grants).await;

    assert!(provider.active_lease(&LeaseId::new(HELD)).await.is_some());
    Ok(())
}

#[tokio::test]
async fn a_cancel_subject_without_a_lease_is_ignored() -> TestResult {
    let (provider, _control, _index) = provider_holding(Router::new(), &[HELD]).await?;
    let grants = RedispatchGuard::new();

    handle_cancel_msg(
        &cancel_message("sandbox.provider.substrate.cancel.", Bytes::new()),
        &provider,
        &grants,
    )
    .await;

    assert!(provider.active_lease(&LeaseId::new(HELD)).await.is_some());
    Ok(())
}

#[tokio::test]
async fn a_cancel_the_provider_cannot_complete_leaves_the_lease_held() -> TestResult {
    let (provider, control, _index) = provider_holding(Router::new(), &[HELD]).await?;
    control.fail_get_with(AteError::Unavailable {
        message: "control plane down".to_string(),
    });
    let grants = RedispatchGuard::new();

    let message = cancel_message(
        &format!("sandbox.provider.substrate.cancel.{HELD}"),
        Bytes::from(serde_json::to_vec(&CancelIntent::Release)?),
    );
    handle_cancel_msg(&message, &provider, &grants).await;

    assert!(provider.active_lease(&LeaseId::new(HELD)).await.is_some());
    Ok(())
}
