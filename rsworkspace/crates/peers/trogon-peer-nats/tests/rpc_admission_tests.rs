use grpc_nats_micro::{ContentType, ServiceFault, constants::HEADER_ERROR_CODE};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use trogon_nats::AdvancedMockNatsClient;
use trogon_peer::TrustedPeerOwner;
use trogon_peer_nats::{PeerAccount, PeerIngress, PeerMethod, PeerRpcAdmission, PeerRpcOperation, PeerTenant};
use trogonai_proto::{nats::micro::v1alpha1::ServiceOptions, peers::v1alpha1 as proto};

#[derive(Default)]
struct Backend(AtomicUsize);
macro_rules! operation {
    ($request:ident, $response:ident) => {
        impl PeerRpcOperation<proto::$request> for Backend {
            type Response = proto::$response;
            async fn execute(
                &self,
                owner: &TrustedPeerOwner,
                _: proto::$request,
            ) -> Result<Self::Response, ServiceFault> {
                assert_eq!(owner.caller().as_str(), "alice");
                assert_eq!(owner.tenant().as_str(), "tenant");
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(proto::$response::default())
            }
        }
    };
}
operation!(EnrollRequest, EnrollResponse);
operation!(HeartbeatRequest, HeartbeatResponse);
operation!(DetachRequest, DetachResponse);
operation!(RosterRequest, RosterResponse);
operation!(SendRequest, SendResponse);
operation!(StatusRequest, StatusResponse);
operation!(PrepareBatchRequest, PrepareBatchResponse);
operation!(AcknowledgeBatchRequest, AcknowledgeBatchResponse);

type TestResult = Result<(), Box<dyn std::error::Error>>;
fn admission(backend: Arc<Backend>) -> Result<PeerRpcAdmission<Backend>, Box<dyn std::error::Error>> {
    Ok(PeerRpcAdmission {
        ingress: PeerIngress::new(PeerAccount::SharedFixture {
            tenant: PeerTenant::new("tenant")?,
        }),
        method: PeerMethod::Enroll,
        backend,
    })
}
fn message(subject: &str, reply: Option<&str>, encoding: &str, payload: Vec<u8>) -> async_nats::Message {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Content-Type", encoding);
    headers.insert("X-Peer-Caller", "forged");
    headers.insert("X-Peer-Tenant", "forged");
    async_nats::Message {
        subject: subject.to_owned().into(),
        reply: reply.map(|r| r.to_owned().into()),
        payload: payload.into(),
        headers: Some(headers),
        status: None,
        description: None,
        length: 0,
    }
}

#[tokio::test]
async fn admitted_subject_supplies_owner_and_shared_codecs_dispatch_protobuf_and_json() -> TestResult {
    let backend = Arc::new(Backend::default());
    let admission = admission(backend.clone())?;
    for encoding in [ContentType::Protobuf, ContentType::Json] {
        let client = AdvancedMockNatsClient::new();
        let message = message(
            "tenant.peer.v1.alice.PeerService.Enroll",
            Some("_INBOX.tenant.alice.connection.nonce"),
            encoding.header_value(),
            encoding.encode(&proto::EnrollRequest::default())?,
        );
        grpc_nats_micro::server::dispatch_admitted(&client, &message, &ServiceOptions::default(), &admission).await;
        assert_eq!(client.published_messages(), ["_INBOX.tenant.alice.connection.nonce"]);
        assert!(client.published_headers()[0].get(HEADER_ERROR_CODE).is_none());
    }
    assert_eq!(backend.0.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn invalid_metadata_drops_before_decoding_negotiation_or_backend() -> TestResult {
    let backend = Arc::new(Backend::default());
    let admission = admission(backend.clone())?;
    for (subject, reply) in [
        ("tenant.peer.v1.alice.PeerService.Enroll", None),
        ("tenant.peer.v1.alice.PeerService.Enroll", Some("outside.reply")),
        (
            "tenant.peer.v1.alice.PeerService.Enroll",
            Some("_INBOX.other.alice.connection.nonce"),
        ),
        (
            "tenant.peer.v1.alice.PeerService.Enroll",
            Some("_INBOX.tenant.bob.connection.nonce"),
        ),
        ("tenant.peer.v1.alice.PeerService.Enroll", Some("_R_.alias")),
        (
            "other.peer.v1.alice.PeerService.Enroll",
            Some("_INBOX.tenant.alice.connection.nonce"),
        ),
        (
            "tenant.peer.v1.alice.PeerService.Unknown",
            Some("_INBOX.tenant.alice.connection.nonce"),
        ),
        (
            "tenant.peer.v1.alice.PeerService.Send",
            Some("_INBOX.tenant.alice.connection.nonce"),
        ),
        ("", Some("_INBOX.tenant.alice.connection.nonce")),
    ] {
        for encoding in ["application/unsafe", "application/protobuf", "application/json"] {
            let client = AdvancedMockNatsClient::new();
            let message = message(subject, reply, encoding, vec![255]);
            grpc_nats_micro::server::dispatch_admitted(&client, &message, &ServiceOptions::default(), &admission).await;
            assert!(client.published_messages().is_empty());
        }
    }
    assert_eq!(backend.0.load(Ordering::SeqCst), 0);
    Ok(())
}
