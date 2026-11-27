mod common;
mod lease_support;

use std::time::Duration;

use common::{MockBus, Reply, TestResult, client_replying, lease_id, tenant};
use lease_support::{sample_granted, sample_request, unix};
use trogon_sandbox_client::{ClientError, SandboxClient};
use trogon_sandbox_contract::{
    ExtendLeaseTimeoutResponse, LeaseError, LeaseResponse, NetworkPolicy, NetworkPolicyUpdateResponse, OperationId,
    SandboxTags, UpdateLeaseTagsResponse,
};

#[tokio::test]
async fn lease_returns_granted_when_broker_grants() -> TestResult {
    let granted = sample_granted();
    let (client, log) = client_replying(&LeaseResponse::Granted(granted.clone()))?;
    let got = client.lease(&sample_request()?, Duration::from_secs(5)).await?;
    assert_eq!(got.lease_id, granted.lease_id);
    assert_eq!(got.sandbox_id, granted.sandbox_id);
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.request");
    assert_eq!(sent.timeout, Duration::from_secs(5));
    Ok(())
}

#[tokio::test]
async fn lease_returns_denied_when_broker_denies_quota_exceeded() -> TestResult {
    let error = LeaseError::QuotaExceeded { tenant: tenant()? };
    let (client, _) = client_replying(&LeaseResponse::Denied(error))?;
    let result = client.lease(&sample_request()?, Duration::from_secs(5)).await;
    assert!(matches!(
        result,
        Err(ClientError::Denied(LeaseError::QuotaExceeded { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn lease_returns_timeout_when_bus_does_not_reply() -> TestResult {
    let client = MockBus::timing_out().into_client();
    let result = client.lease(&sample_request()?, Duration::from_millis(50)).await;
    assert!(matches!(result, Err(ClientError::Timeout(_))));
    Ok(())
}

#[tokio::test]
async fn lease_returns_transport_error_when_bus_fails() -> TestResult {
    let client = MockBus::failing().into_client();
    let result = client.lease(&sample_request()?, Duration::from_secs(1)).await;
    assert!(matches!(result, Err(ClientError::Transport(_))));
    Ok(())
}

#[tokio::test]
async fn lease_returns_transport_error_when_the_reply_is_not_a_lease_response() -> TestResult {
    let client = MockBus::replying("not json".into()).into_client();
    let result = client.lease(&sample_request()?, Duration::from_secs(1)).await;
    assert!(matches!(result, Err(ClientError::Transport(_))));
    Ok(())
}

#[tokio::test]
async fn cancel_succeeds_even_when_bus_drops_message() -> TestResult {
    let bus = MockBus::timing_out();
    let log = bus.log();
    let client = bus.into_client();
    client.cancel(&lease_id()).await?;
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.cancel.lease-1");
    assert_eq!(sent.timeout, Duration::from_secs(2));
    Ok(())
}

#[tokio::test]
async fn api_key_propagated_as_authorization_header() -> TestResult {
    let bus = MockBus::replying_json(&LeaseResponse::Granted(sample_granted()))?;
    let log = bus.log();
    let client = SandboxClient::from_bus(Box::new(bus)).with_api_key("my-secret-key".to_string());
    client.lease(&sample_request()?, Duration::from_secs(5)).await?;
    assert_eq!(log.only()?.authorization().as_deref(), Some("Bearer my-secret-key"));
    Ok(())
}

#[tokio::test]
async fn no_api_key_sends_no_authorization_header() -> TestResult {
    let (client, log) = client_replying(&LeaseResponse::Granted(sample_granted()))?;
    client.lease(&sample_request()?, Duration::from_secs(5)).await?;
    assert!(log.only()?.headers.is_none());
    Ok(())
}

#[tokio::test]
async fn lease_with_operation_sends_the_operation_id_only_when_given() -> TestResult {
    let granted = LeaseResponse::Granted(sample_granted());
    let (client, log) = client_replying(&granted)?;
    client
        .lease_with_operation(
            &sample_request()?,
            Some(OperationId::new("create-1")?),
            Duration::from_secs(1),
        )
        .await?;
    assert_eq!(log.only()?.json()?["operation_id"], "create-1");

    let (client, log) = client_replying(&granted)?;
    client
        .lease_with_operation(&sample_request()?, None, Duration::from_secs(1))
        .await?;
    assert!(log.only()?.json()?.get("operation_id").is_none());
    Ok(())
}

#[tokio::test]
async fn update_tags_applies_and_maps_denial_and_timeout() -> TestResult {
    let (client, log) = client_replying(&UpdateLeaseTagsResponse::Applied)?;
    client.update_tags(&lease_id(), SandboxTags::default()).await?;
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.tags.update.lease-1");
    assert_eq!(sent.timeout, Duration::from_secs(2));

    let denied = UpdateLeaseTagsResponse::Denied(LeaseError::LeaseNotFound { lease_id: lease_id() });
    let (client, _) = client_replying(&denied)?;
    let result = client.update_tags(&lease_id(), SandboxTags::default()).await;
    assert!(matches!(
        result,
        Err(ClientError::Denied(LeaseError::LeaseNotFound { .. }))
    ));

    let client = MockBus::timing_out().into_client();
    let result = client.update_tags(&lease_id(), SandboxTags::default()).await;
    assert!(matches!(result, Err(ClientError::Timeout(_))));
    Ok(())
}

#[tokio::test]
async fn update_network_policy_succeeds_when_broker_applies() -> TestResult {
    let (client, _) = client_replying(&NetworkPolicyUpdateResponse::Applied)?;
    client
        .update_network_policy(&lease_id(), NetworkPolicy::DenyAll)
        .await?;
    Ok(())
}

#[tokio::test]
async fn update_network_policy_returns_denied_when_broker_denies() -> TestResult {
    let error = LeaseError::LeaseNotRunning { lease_id: lease_id() };
    let (client, _) = client_replying(&NetworkPolicyUpdateResponse::Denied(error))?;
    let result = client.update_network_policy(&lease_id(), NetworkPolicy::DenyAll).await;
    assert!(matches!(
        result,
        Err(ClientError::Denied(LeaseError::LeaseNotRunning { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn extend_timeout_returns_new_deadline_when_applied() -> TestResult {
    let new_deadline = unix(600);
    let (client, _) = client_replying(&ExtendLeaseTimeoutResponse::Extended { new_deadline })?;
    assert_eq!(client.extend_timeout(&lease_id(), 300).await?, new_deadline);
    Ok(())
}

#[tokio::test]
async fn extend_timeout_returns_denied_when_broker_denies() -> TestResult {
    let error = LeaseError::LeaseNotRunning { lease_id: lease_id() };
    let (client, _) = client_replying(&ExtendLeaseTimeoutResponse::Denied(error))?;
    let result = client.extend_timeout(&lease_id(), 300).await;
    assert!(matches!(
        result,
        Err(ClientError::Denied(LeaseError::LeaseNotRunning { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn a_scripted_timeout_then_a_reply_is_served_in_order() -> TestResult {
    let bus = MockBus::scripted(vec![
        Reply::Timeout,
        Reply::json(&LeaseResponse::Granted(sample_granted()))?,
    ]);
    let client = bus.into_client();
    let first = client.lease(&sample_request()?, Duration::from_secs(1)).await;
    assert!(matches!(first, Err(ClientError::Timeout(_))));
    client.lease(&sample_request()?, Duration::from_secs(1)).await?;
    Ok(())
}
