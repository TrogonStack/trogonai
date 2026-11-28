mod common;

use std::time::Duration;

use bytes::Bytes;
use common::{MockBus, TestResult, lease_id, tenant};
use trogon_sandbox_client::{BusMessage, ClientError, TransportError};
use trogon_sandbox_contract::{LeaseReleased, LeaseReleasedReason};

fn released(reason: LeaseReleasedReason) -> Result<BusMessage, Box<dyn std::error::Error>> {
    let event = LeaseReleased {
        lease_id: lease_id(),
        tenant: tenant()?,
        reason,
        snapshot_out: None,
        worker_id: None,
        correlation_id: None,
        restored_from: None,
        released_at: None,
    };
    Ok(BusMessage {
        subject: "sandbox.lease.released.lease-1".to_string(),
        payload: Bytes::from(serde_json::to_vec(&event)?),
    })
}

#[tokio::test]
async fn wait_released_returns_once_the_lease_is_released() -> TestResult {
    let client = MockBus::with_subscribe_items(vec![released(LeaseReleasedReason::Expired)?]).into_client();
    client.wait_released(&lease_id(), Duration::from_secs(5)).await?;
    Ok(())
}

#[tokio::test]
async fn wait_released_with_reason_reports_why() -> TestResult {
    let client = MockBus::with_subscribe_items(vec![released(LeaseReleasedReason::Cancelled)?]).into_client();
    let reason = client
        .wait_released_with_reason(&lease_id(), Duration::from_secs(5))
        .await?;
    assert_eq!(reason, LeaseReleasedReason::Cancelled);
    Ok(())
}

#[tokio::test]
async fn an_undecodable_announcement_is_a_decode_error_not_completed() -> TestResult {
    let message = BusMessage {
        subject: "sandbox.lease.released.lease-1".to_string(),
        payload: Bytes::from("not json"),
    };
    let client = MockBus::with_subscribe_items(vec![message]).into_client();
    let result = client
        .wait_released_with_reason(&lease_id(), Duration::from_secs(5))
        .await;
    assert!(matches!(result, Err(ClientError::Transport(TransportError::Decode(_)))));
    Ok(())
}

#[tokio::test]
async fn a_closed_subscription_reads_as_a_timeout() -> TestResult {
    let client = MockBus::with_subscribe_items(vec![]).into_client();
    let result = client.wait_released(&lease_id(), Duration::from_secs(5)).await;
    assert!(matches!(result, Err(ClientError::Timeout(_))));
    Ok(())
}
