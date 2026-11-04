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
        assert_eq!(response.attachment.attachment.as_option(), Some(&fixture.a));
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

#[tokio::test]
async fn service_discovery_registers_the_annotated_methods_and_queue() -> TestResult {
    let fixture = Fixture::new().await?;
    let backend = std::sync::Arc::new(PeerRpcApplication::new(
        PeerService::new(fixture.store.clone(), fixture.policy.clone(), fixture.clock.clone()),
        fixture.app(),
    ));
    let broker = trogon_nats::test_support::JetStreamTestServer::start_with_atomic_batches().await;
    let client = broker.client().await;
    let account = trogon_peer_nats::PeerAccount::SharedFixture {
        tenant: trogon_peer_nats::PeerTenant::new("tenant")?,
    };
    let _service = trogon_peer_nats::serve_peer_rpc(&client, account, backend).await?;
    let response = client.request("$SRV.INFO.PeerService", Vec::new().into()).await?;
    let info: serde_json::Value = serde_json::from_slice(&response.payload)?;
    assert_eq!(info["version"], "0.1.0");
    assert_eq!(info["description"], "Coordinates enrolled peer communication");
    let endpoints = info["endpoints"].as_array().ok_or("no endpoints")?;
    assert_eq!(endpoints.len(), 8);
    let mut names = Vec::new();
    for endpoint in endpoints {
        let name = endpoint["name"].as_str().ok_or("no method")?;
        assert_eq!(endpoint["subject"], format!("tenant.peer.v1.*.PeerService.{name}"));
        assert_eq!(endpoint["queue_group"], "q");
        names.push(name);
    }
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "AcknowledgeBatch",
            "Detach",
            "Enroll",
            "Heartbeat",
            "PrepareBatch",
            "Roster",
            "Send",
            "Status"
        ]
    );
    Ok(())
}
