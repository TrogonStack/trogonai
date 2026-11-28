use std::error::Error;

use trogon_sandbox_contract::{ProviderLease, SandboxId};
use trogon_sandbox_protocol::ProviderError;

use super::*;

mod authority_pause_tests;
mod docker_lease;
mod heartbeat_identity_tests;
mod lease_request;
mod mock_provider;
mod named_cancellation_tests;
mod named_origin_tests;
mod redispatch_tests;
mod run_command_decode_tests;

use lease_request::sample_lease_request;
use mock_provider::{MockOutcome, MockProvider};

type TestResult = Result<(), Box<dyn Error>>;

#[tokio::test]
async fn successful_lease_returns_provider_lease() -> TestResult {
    let granted = ProviderLease {
        epoch: None,
        sandbox_id: SandboxId::new("container-abc")?,
        evidence_subject: "sandbox.evidence.01J.>".to_owned(),
        worker_id: None,
        attempt_id: None,
    };
    let provider = Arc::new(MockProvider {
        outcome: MockOutcome::Grant(granted.clone()),
    });
    let lease = process_lease_request(sample_lease_request()?, provider).await?;
    assert_eq!(lease, granted);
    Ok(())
}

#[tokio::test]
async fn failed_lease_propagates_the_backend_error() -> TestResult {
    let provider = Arc::new(MockProvider {
        outcome: MockOutcome::Fail,
    });
    let result = process_lease_request(sample_lease_request()?, provider).await;
    let Err(error) = result else {
        return Err("a failed backend must not grant a lease".into());
    };
    assert!(matches!(error, ProviderError::Backend(_)), "{error}");
    Ok(())
}

#[test]
fn extract_cancel_lease_id_parses_last_segment() {
    let lease_id = extract_cancel_lease_id("sandbox.provider.local_docker.cancel.01JTEST0000000000000000000");
    assert_eq!(lease_id, Some(LeaseId::new("01JTEST0000000000000000000")));
}

#[test]
fn extract_cancel_lease_id_returns_none_for_empty_trailing_segment() {
    assert_eq!(extract_cancel_lease_id("sandbox.provider.local_docker.cancel."), None);
}

#[test]
fn decode_error_reply_carries_the_reason_under_its_wire_key() -> TestResult {
    let reply = DecodeErrorReply {
        decode_error: "bad payload".to_owned(),
    };
    let wire: serde_json::Value = serde_json::from_slice(&serde_json::to_vec(&reply)?)?;
    assert_eq!(wire.get("decode_error").and_then(|v| v.as_str()), Some("bad payload"));
    Ok(())
}
