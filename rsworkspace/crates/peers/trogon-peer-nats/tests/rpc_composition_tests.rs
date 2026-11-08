use futures::StreamExt as _;
use grpc_nats_micro::ServiceFault;
use trogon_peer_nats::{PeerRpcApplication, PeerRpcOperation};
use trogon_peer_service::{PeerService, test_support::Fixture};
use trogonai_proto::peers::v1alpha1 as proto;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn admitted_success_is_counted_in_micro_statistics() -> TestResult {
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
    let reply = "_INBOX.tenant.alice.connection.statistics";
    let mut incoming = client.subscribe(reply).await?;
    client
        .publish_with_reply(
            "tenant.peer.v1.alice.PeerService.Enroll",
            reply,
            grpc_nats_micro::ContentType::Protobuf
                .encode(&Fixture::enrollment("alice"))?
                .into(),
        )
        .await?;
    let response = tokio::time::timeout(std::time::Duration::from_secs(2), incoming.next())
        .await?
        .ok_or("no enrollment response")?;
    let _: proto::EnrollResponse = grpc_nats_micro::status_codec::decode_reply(
        response.headers.as_ref(),
        &response.payload,
        grpc_nats_micro::ContentType::Protobuf,
    )?;
    let stats = client.request("$SRV.STATS.PeerService", Vec::new().into()).await?;
    let stats: serde_json::Value = serde_json::from_slice(&stats.payload)?;
    let enrollment = stats["endpoints"]
        .as_array()
        .ok_or("no endpoint statistics")?
        .iter()
        .find(|endpoint| endpoint["name"] == "Enroll")
        .ok_or("no enrollment statistics")?;
    assert_eq!(
        enrollment["num_requests"], 1,
        "successful admitted traffic must remain observable"
    );
    assert!(
        enrollment["processing_time"]
            .as_u64()
            .is_some_and(|elapsed| elapsed > 0)
    );
    Ok(())
}

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

#[tokio::test]
async fn named_peer_registration_preserves_the_existing_shared_prefix_boundary() -> TestResult {
    use futures::StreamExt as _;
    use grpc_nats_micro::SubjectPrefixError;
    use std::sync::{Arc, atomic::Ordering};
    use std::time::Duration;
    use trogon_peer_nats::{PeerAccount, PeerRpcServeError, PeerTenant, serve_peer_rpc};

    let fixture = Fixture::new().await?;
    let backend = Arc::new(PeerRpcApplication::new(
        PeerService::new(fixture.store.clone(), fixture.policy.clone(), fixture.clock.clone()),
        fixture.app(),
    ));
    let broker = trogon_nats::test_support::JetStreamTestServer::start_with_atomic_batches().await;
    let client = broker.client().await;
    let reads = fixture.store.reads.load(Ordering::SeqCst);
    let appends = fixture.store.appends.load(Ordering::SeqCst);
    let accepted = "a".repeat(120);
    let service = tokio::time::timeout(
        Duration::from_secs(2),
        serve_peer_rpc(
            &client,
            PeerAccount::SharedFixture {
                tenant: PeerTenant::new(accepted.clone())?,
            },
            Arc::clone(&backend),
        ),
    )
    .await??;
    let barrier = client.new_inbox();
    let mut observed = client.subscribe(barrier.clone()).await?;
    client.publish(barrier, vec![0].into()).await?;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), observed.next())
            .await?
            .ok_or("peer registration barrier was not observed")?
            .payload
            .as_ref(),
        &[0]
    );
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        client.request("$SRV.INFO.PeerService", Vec::new().into()),
    )
    .await??;
    let info: serde_json::Value = serde_json::from_slice(&response.payload)?;
    let endpoints = info["endpoints"].as_array().ok_or("missing peer endpoints")?;
    assert_eq!(endpoints.len(), 8);
    for endpoint in endpoints {
        let method = endpoint["name"].as_str().ok_or("missing peer method")?;
        assert_eq!(
            endpoint["subject"],
            format!("{accepted}.peer.v1.*.PeerService.{method}")
        );
        assert_eq!(endpoint["queue_group"], "q");
    }
    tokio::time::timeout(Duration::from_secs(2), service.stop()).await??;
    for length in 121..=128 {
        let tenant = PeerTenant::new("a".repeat(length))?;
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            serve_peer_rpc(&client, PeerAccount::SharedFixture { tenant }, Arc::clone(&backend)),
        )
        .await?;
        let error = result.err().ok_or("oversized shared prefix registered")?;
        assert!(
            matches!(error, PeerRpcServeError::Prefix(SubjectPrefixError::TooLong(bytes)) if bytes == length + ".peer.v1".len())
        );
    }
    assert_eq!(fixture.store.reads.load(Ordering::SeqCst), reads);
    assert_eq!(fixture.store.appends.load(Ordering::SeqCst), appends);
    Ok(())
}
