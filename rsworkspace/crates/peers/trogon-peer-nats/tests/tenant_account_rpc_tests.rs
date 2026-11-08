use std::{sync::Arc, time::Duration};

use futures::StreamExt as _;
use grpc_nats_micro::ContentType;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_peer_nats::{
    PeerAccount, PeerCaller, PeerConnectionId, PeerRequestTimeout, PeerRequester, PeerRequesterConfig,
    PeerRpcApplication, PeerRpcClient, PeerTenant, serve_peer_rpc,
};
use trogon_peer_service::{PeerService, test_support::Fixture};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[tokio::test]
async fn tenant_account_rpc_uses_account_scope_without_a_tenant_subject_prefix() -> TestResult {
    let fixture = Fixture::new().await?;
    let backend = Arc::new(PeerRpcApplication::new(
        PeerService::new(fixture.store.clone(), fixture.policy.clone(), fixture.clock.clone()),
        fixture.app(),
    ));
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let service = broker.client().await;
    let account = PeerAccount::TenantAccount {
        tenant: PeerTenant::new("tenant")?,
    };
    let _registration = serve_peer_rpc(&service, account.clone(), backend).await?;
    let subject = service.new_inbox();
    let mut observed = service.subscribe(subject.clone()).await?;
    service.publish(subject, vec![0].into()).await?;
    tokio::time::timeout(Duration::from_secs(2), observed.next())
        .await?
        .ok_or("registration barrier missing")?;
    let rpc = PeerRpcClient::new(
        PeerRequester::new(
            broker.client().await,
            PeerRequesterConfig::new(
                account,
                PeerCaller::new("alice")?,
                PeerConnectionId::new("connection")?,
                PeerRequestTimeout::new(Duration::from_secs(1))?,
            ),
        ),
        ContentType::Protobuf,
    );
    let enrolled = rpc.enroll(&Fixture::enrollment("alice")).await?;
    assert_eq!(enrolled.attachment.attachment_ref.as_option(), Some(&fixture.a));
    let info = service.request("$SRV.INFO.PeerService", Vec::new().into()).await?;
    let info: serde_json::Value = serde_json::from_slice(&info.payload)?;
    let endpoints = info["endpoints"].as_array().ok_or("missing endpoints")?;
    assert_eq!(endpoints.len(), 8);
    for endpoint in endpoints {
        let method = endpoint["name"].as_str().ok_or("missing method")?;
        assert_eq!(endpoint["subject"], format!("peer.v1.*.PeerService.{method}"));
        assert_eq!(endpoint["queue_group"], "q");
    }
    Ok(())
}
