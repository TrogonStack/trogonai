use std::sync::Arc;
use std::time::Duration;

use grpc_nats_micro::{ContentType, ReplyError};
use trogon_peer::PeerId;
use trogon_peer_mcp::{HeartbeatPeer, RpcPeerBackend};
use trogon_peer_nats::test_support::broker::{Broker, TestResult, observed_barrier};
use trogon_peer_nats::{
    PeerAccount, PeerCaller, PeerConnectionId, PeerRequestTimeout, PeerRequester, PeerRequesterConfig,
    PeerRpcApplication, PeerRpcClient, PeerRpcClientError, PeerTenant, serve_peer_rpc,
};
use trogon_peer_service::{PeerService, test_support::Fixture};
use trogonai_proto::peers::v1alpha1 as v1;

#[tokio::test]
async fn rpc_backend_heartbeat_updates_only_the_authenticated_attachment_and_preserves_typed_faults() -> TestResult {
    let broker = Broker::start().await?;
    let (service, _) = broker.connect("service").await?;
    let (alice, _) = broker.connect("alice").await?;
    let fixture = Fixture::new().await?;
    let application = Arc::new(PeerRpcApplication::new(
        PeerService::new(fixture.store.clone(), fixture.policy.clone(), fixture.clock.clone()),
        fixture.app(),
    ));
    let account = PeerAccount::SharedFixture {
        tenant: PeerTenant::new("tenant")?,
    };
    let registration = serve_peer_rpc(&service, account.clone(), application).await?;
    observed_barrier(&service).await?;
    let backend = RpcPeerBackend::new(PeerRpcClient::new(
        PeerRequester::new(
            alice,
            PeerRequesterConfig::new(
                account,
                PeerCaller::new("alice")?,
                PeerConnectionId::new("one")?,
                PeerRequestTimeout::new(Duration::from_secs(2))?,
            ),
        ),
        ContentType::Protobuf,
    ));
    let diagnostic = format!("{backend:?}");
    assert!(
        !diagnostic.contains("alice"),
        "backend Debug exposed authenticated caller routing"
    );
    assert!(
        !diagnostic.contains(broker.server.address()),
        "backend Debug exposed connection details"
    );
    let peer = PeerId::parse(fixture.a.peer_id.clone().ok_or("fixture sender absent")?)?;
    let response = backend
        .heartbeat(v1::HeartbeatRequest {
            attachment_ref: fixture.a.clone().into(),
            availability: Some(v1::PeerAvailability::PEER_AVAILABILITY_BLOCKED.into()),
            ..Default::default()
        })
        .await?;
    assert_eq!(response.attachment.attachment_ref.as_option(), Some(&fixture.a));
    let observed = fixture.history(&peer).await?;
    assert_eq!(
        observed.availability.as_ref().and_then(|value| value.as_known()),
        Some(v1::PeerAvailability::PEER_AVAILABILITY_BLOCKED)
    );
    assert!(!observed.active_at.is_set(), "heartbeat must not fabricate progress");
    let invalid = backend
        .heartbeat(v1::HeartbeatRequest {
            attachment_ref: fixture.a.clone().into(),
            ..Default::default()
        })
        .await;
    let error = invalid.err().ok_or("missing availability was admitted")?;
    let PeerRpcClientError::Reply(ReplyError::Service(fault)) = error else {
        return Err("invalid heartbeat lost its typed service fault".into());
    };
    assert_eq!(fault.status().code, 3);
    assert_eq!(fault.message(), "invalid peer request");
    assert!(fault.status().details.is_empty());
    assert_eq!(
        fixture.history(&peer).await?,
        observed,
        "invalid heartbeat mutated authoritative state"
    );
    registration.stop().await?;
    Ok(())
}
