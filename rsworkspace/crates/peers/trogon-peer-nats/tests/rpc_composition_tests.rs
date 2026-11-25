use grpc_nats_micro::ServiceFault;
use trogon_peer_nats::{PeerRpcApplication, PeerRpcOperation};
use trogon_peer_service::{PeerService, test_support::Fixture};
use trogonai_proto::peers::v1alpha1 as proto;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn real_service_composition_preserves_resume_and_scoped_policy() -> TestResult {
    let fixture = Fixture::new().await?;
    let adapter = PeerRpcApplication::new(
        PeerService::new(fixture.store.clone(), fixture.policy.clone(), fixture.clock.clone()),
        fixture.app(),
    );
    let result =
        PeerRpcOperation::<proto::EnrollRequest>::execute(&adapter, &fixture.alice, Fixture::enrollment("alice")).await;
    assert!(result.is_ok(), "composition must execute real enrollment");
    if let Ok(response) = result {
        assert_eq!(response.attachment.attachment_ref.as_option(), Some(&fixture.a));
    }
    fixture.policy.deny.store(true, std::sync::atomic::Ordering::SeqCst);
    let result =
        PeerRpcOperation::<proto::EnrollRequest>::execute(&adapter, &fixture.alice, Fixture::enrollment("alice")).await;
    assert_eq!(result.err().map(|fault| fault.status().code), Some(7));
    Ok(())
}

#[tokio::test]
async fn malformed_application_inputs_produce_sanitized_canonical_status() -> TestResult {
    let fixture = Fixture::new().await?;
    let adapter = PeerRpcApplication::new(
        PeerService::new(fixture.store.clone(), fixture.policy.clone(), fixture.clock.clone()),
        fixture.app(),
    );
    let result: Result<proto::SendResponse, ServiceFault> =
        adapter.execute(&fixture.alice, proto::SendRequest::default()).await;
    let error = result.err().ok_or("invalid request succeeded")?;
    assert_eq!(error.status().code, 3);
    assert_eq!(error.message(), "invalid peer request");
    assert!(error.status().details.is_empty());
    Ok(())
}
