#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fmt::Debug;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::HeaderMap;
use buffa::{Enumeration as _, Message, MessageField};
use buffa_types::google::protobuf::{Any, Timestamp};
use grpc_nats_micro::constants::{HEADER_CONTENT_TYPE, HEADER_ERROR, HEADER_ERROR_CODE};
use grpc_nats_micro::server::dispatch_admitted;
use grpc_nats_micro::status_codec::{self, ReplyError};
use grpc_nats_micro::{ContentType, ServiceFault};
use serde::{Serialize, de::DeserializeOwned};
use trogon_nats::AdvancedMockNatsClient;
use trogon_peer::{CallerId, TenantId, TrustedPeerOwner};
use trogon_peer_nats::{PeerAccount, PeerIngress, PeerMethod, PeerRpcAdmission, PeerRpcOperation, PeerTenant};
use trogonai_proto::google::rpc::{Code, ErrorInfo, Status};
use trogonai_proto::{nats::micro::v1alpha1::ServiceOptions, peers::v1alpha1 as proto};

type InlineField<T> = MessageField<T, buffa::Inline<T>>;

const TENANT: &str = "authenticated_tenant";
const CALLER: &str = "authenticated_caller";
const REPLY: &str = "_INBOX.authenticated_tenant.authenticated_caller.connection.nonce";
const SENDER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const RECIPIENT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const BOUND: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq)]
enum RequestWitness {
    Enroll(proto::EnrollRequest),
    Heartbeat(proto::HeartbeatRequest),
    Detach(proto::DetachRequest),
    Roster(proto::RosterRequest),
    Send(proto::SendRequest),
    Status(proto::StatusRequest),
    PrepareBatch(proto::PrepareBatchRequest),
    AcknowledgeBatch(proto::AcknowledgeBatchRequest),
}

#[derive(Debug, Clone, PartialEq)]
struct RecordedCall {
    owner: TrustedPeerOwner,
    request: RequestWitness,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BackendOutcome {
    Response,
    Fault,
}

struct Backend {
    expected: RequestWitness,
    owner: TrustedPeerOwner,
    outcome: BackendOutcome,
    calls: Mutex<Vec<RecordedCall>>,
}

impl Backend {
    fn record(&self, owner: &TrustedPeerOwner, request: RequestWitness) {
        assert_eq!(owner, &self.owner, "authenticated principal changed at dispatch");
        assert_eq!(request, self.expected, "wrong backend route or decoded request");
        self.calls.lock().expect("backend call log").push(RecordedCall {
            owner: owner.clone(),
            request,
        });
    }
}

macro_rules! operation {
    ($request:ident, $response:ident, $method:ident, $reply:ident) => {
        impl PeerRpcOperation<proto::$request> for Backend {
            type Response = proto::$response;

            async fn execute(
                &self,
                owner: &TrustedPeerOwner,
                request: proto::$request,
            ) -> Result<Self::Response, ServiceFault> {
                self.record(owner, RequestWitness::$method(request));
                if self.outcome == BackendOutcome::Fault {
                    return Err(backend_fault(PeerMethod::$method));
                }
                Ok($reply())
            }
        }
    };
}

operation!(EnrollRequest, EnrollResponse, Enroll, enroll_response);
operation!(HeartbeatRequest, HeartbeatResponse, Heartbeat, heartbeat_response);
operation!(RosterRequest, RosterResponse, Roster, roster_response);
operation!(SendRequest, SendResponse, Send, send_response);
operation!(StatusRequest, StatusResponse, Status, status_response);
operation!(
    PrepareBatchRequest,
    PrepareBatchResponse,
    PrepareBatch,
    prepare_response
);
operation!(
    AcknowledgeBatchRequest,
    AcknowledgeBatchResponse,
    AcknowledgeBatch,
    acknowledge_response
);

impl PeerRpcOperation<proto::DetachRequest> for Backend {
    type Response = proto::DetachResponse;

    async fn execute(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::DetachRequest,
    ) -> Result<Self::Response, ServiceFault> {
        self.record(owner, RequestWitness::Detach(request));
        Err(backend_fault(PeerMethod::Detach))
    }
}

fn owner() -> TrustedPeerOwner {
    TrustedPeerOwner::new(
        TenantId::new(TENANT).expect("tenant"),
        CallerId::new(CALLER).expect("caller"),
    )
}

fn attachment(method: &str) -> InlineField<proto::AttachmentRef> {
    MessageField::some(proto::AttachmentRef {
        peer_id: Some(SENDER.to_owned()),
        connector_id: Some(format!("connector_{method}")),
        generation: Some(7),
    })
}

fn at(seconds: i64) -> InlineField<Timestamp> {
    MessageField::some(Timestamp {
        seconds,
        nanos: 123,
        ..Default::default()
    })
}

fn lease(method: &str) -> InlineField<proto::PeerAttachment> {
    MessageField::some(proto::PeerAttachment {
        attachment_ref: attachment(method),
        status: Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED.into()),
        observed_at: at(1_700_000_000),
        expires_at: at(1_700_000_010),
    })
}

fn metadata(identity: &str, integrity_byte: u8, semantic_byte: u8) -> InlineField<proto::MessageMetadata> {
    MessageField::some(proto::MessageMetadata {
        message_id: Some(identity.to_owned()),
        sender_peer_id: Some(SENDER.to_owned()),
        recipient_peer_id: Some(RECIPIENT.to_owned()),
        collaboration_ref: Some("matrix_collaboration".to_owned()),
        content_size_bytes: Some(13),
        accepted_at: at(1_700_000_000),
        expires_at: at(1_700_003_600),
        content_ref: MessageField::some(proto::ClaimCheck {
            storage_ref: MessageField::some(proto::StorageLocator {
                value: Some(format!("claim_{identity}")),
            }),
            digest: MessageField::some(proto::Digest {
                algorithm: Some("sha256".to_owned()),
                value: Some(vec![integrity_byte; 32]),
            }),
        }),
        semantic_fingerprint: MessageField::some(proto::ContentSemanticFingerprint {
            value: Some(vec![semantic_byte; 32]),
        }),
        ..Default::default()
    })
}

fn batch(identity: &str) -> InlineField<proto::PeerBatch> {
    MessageField::some(proto::PeerBatch {
        batch_id: Some(identity.to_owned()),
        message_ids: vec!["assigned_message".to_owned()],
        prepared_at: at(1_700_000_001),
        ..Default::default()
    })
}

fn enroll_response() -> proto::EnrollResponse {
    proto::EnrollResponse {
        peer_id: Some(SENDER.to_owned()),
        attachment: lease("enroll"),
    }
}

fn heartbeat_response() -> proto::HeartbeatResponse {
    proto::HeartbeatResponse {
        attachment: lease("heartbeat"),
    }
}

fn roster_response() -> proto::RosterResponse {
    proto::RosterResponse {
        peers: vec![proto::PeerPresence {
            peer_id: Some(RECIPIENT.to_owned()),
            display_name: Some("roster_member".to_owned()),
            availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
            observed_at: at(1_700_000_002),
            expires_at: at(1_700_000_012),
            ..Default::default()
        }],
        next_page_token: Some("roster_next_page".to_owned()),
    }
}

fn send_response() -> proto::SendResponse {
    proto::SendResponse {
        message: metadata("accepted_send_key", 0x11, 0xa1),
    }
}

fn status_response() -> proto::StatusResponse {
    proto::StatusResponse {
        message: metadata("queried_status_key", 0x22, 0xb2),
        status: Some(proto::MessageStatus::MESSAGE_STATUS_INTAKE_RECORDED.into()),
        observed_at: at(1_700_000_003),
    }
}

fn prepare_response() -> proto::PrepareBatchResponse {
    proto::PrepareBatchResponse {
        batch: batch("prepared_prompt_identity"),
        messages: vec![proto::PreparedMessage {
            metadata: metadata("prepared_message", 0x33, 0xc3),
            text: Some("retained body".to_owned()),
        }],
        content_availability: Some(proto::ContentAvailability::CONTENT_AVAILABILITY_AVAILABLE.into()),
    }
}

fn acknowledge_response() -> proto::AcknowledgeBatchResponse {
    proto::AcknowledgeBatchResponse {
        batch: MessageField::some(proto::PeerBatch {
            batch_id: Some("acknowledged_batch".to_owned()),
            message_ids: vec!["assigned_message".to_owned()],
            prepared_at: at(1_700_000_001),
            acknowledged_at: at(1_700_000_004),
        }),
    }
}

fn backend_fault(method: PeerMethod) -> ServiceFault {
    let code = match method {
        PeerMethod::Enroll => Code::INVALID_ARGUMENT,
        PeerMethod::Heartbeat => Code::FAILED_PRECONDITION,
        PeerMethod::Detach => Code::PERMISSION_DENIED,
        PeerMethod::Roster => Code::NOT_FOUND,
        PeerMethod::Send => Code::RESOURCE_EXHAUSTED,
        PeerMethod::Status => Code::ABORTED,
        PeerMethod::PrepareBatch => Code::UNAVAILABLE,
        PeerMethod::AcknowledgeBatch => Code::DEADLINE_EXCEEDED,
    };
    let detail = ErrorInfo {
        reason: format!("BACKEND_{}", method.as_str()),
        domain: "peer.rpc.admission.matrix".to_owned(),
        ..Default::default()
    };
    ServiceFault::new(Status {
        code: code.to_i32(),
        message: format!("{} backend outcome", method.as_str()),
        details: vec![Any::pack(&detail, ErrorInfo::TYPE_URL)],
    })
    .expect("canonical non-success status")
}

async fn assert_route<Request, Response>(
    method: PeerMethod,
    request: Request,
    witness: RequestWitness,
    expected_response: Option<Response>,
    encoding: ContentType,
    outcome: BackendOutcome,
) where
    Request: Message + Serialize,
    Response: Message + DeserializeOwned + Debug + PartialEq,
{
    let backend = Arc::new(Backend {
        expected: witness.clone(),
        owner: owner(),
        outcome,
        calls: Mutex::new(Vec::new()),
    });
    let admission = PeerRpcAdmission {
        ingress: PeerIngress::new(PeerAccount::SharedFixture {
            tenant: PeerTenant::new(TENANT).expect("account tenant"),
        }),
        method,
        backend: backend.clone(),
    };
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_CONTENT_TYPE, encoding.header_value());
    headers.insert("X-Peer-Caller", "forged_caller");
    headers.insert("X-Peer-Tenant", "forged_tenant");
    let payload = encoding.encode(&request).expect("encode route-specific request");
    let message = async_nats::Message {
        subject: format!("{TENANT}.peer.v1.{CALLER}.PeerService.{}", method.as_str()).into(),
        reply: Some(REPLY.into()),
        length: payload.len(),
        payload: payload.into(),
        headers: Some(headers),
        status: None,
        description: None,
    };
    let client = AdvancedMockNatsClient::new();
    tokio::time::timeout(
        BOUND,
        dispatch_admitted(&client, &message, &ServiceOptions::default(), &admission),
    )
    .await
    .expect("public admitted dispatch stayed bounded");
    assert_eq!(
        *backend.calls.lock().expect("backend calls"),
        vec![RecordedCall {
            owner: owner(),
            request: witness,
        }],
        "one exact typed backend invocation is required"
    );
    assert_eq!(client.published_messages(), vec![REPLY.to_owned()]);
    let published_headers = client.published_headers();
    let published_payloads = client.published_payloads();
    assert_eq!(published_headers.len(), 1);
    assert_eq!(published_payloads.len(), 1);
    let headers = &published_headers[0];
    let payload = &published_payloads[0];
    assert_eq!(
        headers.get(HEADER_CONTENT_TYPE).expect("reply content type").as_str(),
        encoding.header_value()
    );
    if outcome == BackendOutcome::Response
        && let Some(expected) = expected_response
    {
        assert!(headers.get(HEADER_ERROR_CODE).is_none());
        assert!(headers.get(HEADER_ERROR).is_none());
        let actual: Response =
            status_codec::decode_reply(Some(headers), payload, encoding).expect("decode route-specific response");
        assert_eq!(
            actual, expected,
            "response identities must survive the codec and dispatch"
        );
        return;
    }
    let expected = backend_fault(method);
    assert_eq!(
        headers.get(HEADER_ERROR_CODE).expect("typed error code").as_str(),
        expected.code().to_i32().to_string()
    );
    assert_eq!(
        headers.get(HEADER_ERROR).expect("fault message").as_str(),
        expected.message()
    );
    let error = status_codec::decode_reply::<Response>(Some(headers), payload, encoding)
        .expect_err("backend fault must not become an empty success");
    match error {
        ReplyError::Service(error) => {
            assert_eq!(error.code(), expected.code());
            assert_eq!(
                error.status(),
                expected.status(),
                "the entire typed status and detail must be preserved"
            );
            assert_eq!(error.message(), expected.message());
            let detail = error.status().details[0]
                .unpack_if::<ErrorInfo>(ErrorInfo::TYPE_URL)
                .expect("decode ErrorInfo")
                .expect("backend ErrorInfo detail");
            assert_eq!(detail.reason, format!("BACKEND_{}", method.as_str()));
            assert_eq!(detail.domain, "peer.rpc.admission.matrix");
        }
        other => panic!("backend fault changed category: {other:?}"),
    }
}

macro_rules! check {
    ($encoding:expr, $outcome:expr, $method:ident, $request:expr, $response:expr) => {
        let request = $request;
        let witness = RequestWitness::$method(request.clone());
        assert_route(
            PeerMethod::$method,
            request,
            witness,
            Some($response),
            $encoding,
            $outcome,
        )
        .await;
    };
}

async fn assert_matrix(encoding: ContentType, outcome: BackendOutcome) {
    check!(
        encoding,
        outcome,
        Enroll,
        proto::EnrollRequest {
            installation_id: Some("installation_enroll".to_owned()),
            native_session_id: Some("native_session_enroll".to_owned()),
            connector_id: Some("connector_enroll".to_owned()),
            collaboration_ref: Some("matrix_collaboration".to_owned()),
            display_name: Some("enrolled_peer".to_owned()),
        },
        enroll_response()
    );
    check!(
        encoding,
        outcome,
        Heartbeat,
        proto::HeartbeatRequest {
            attachment_ref: attachment("heartbeat"),
            availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
            active_at: at(1_700_000_000),
            ..Default::default()
        },
        heartbeat_response()
    );
    let detach = proto::DetachRequest {
        attachment_ref: attachment("detach"),
    };
    assert_route::<_, proto::DetachResponse>(
        PeerMethod::Detach,
        detach.clone(),
        RequestWitness::Detach(detach),
        None,
        encoding,
        outcome,
    )
    .await;
    check!(
        encoding,
        outcome,
        Roster,
        proto::RosterRequest {
            attachment_ref: attachment("roster"),
            page_size: Some(3),
            page_token: Some("roster_previous_page".to_owned()),
        },
        roster_response()
    );
    check!(
        encoding,
        outcome,
        Send,
        proto::SendRequest {
            attachment_ref: attachment("send"),
            recipient_peer_id: Some(RECIPIENT.to_owned()),
            idempotency_key: Some("send_key".to_owned()),
            text: Some("message π body".to_owned()),
            reply_to_message_id: Some("prior_message".to_owned()),
        },
        send_response()
    );
    check!(
        encoding,
        outcome,
        Status,
        proto::StatusRequest {
            attachment_ref: attachment("status"),
            idempotency_key: Some("status_key".to_owned()),
        },
        status_response()
    );
    check!(
        encoding,
        outcome,
        PrepareBatch,
        proto::PrepareBatchRequest {
            attachment_ref: attachment("prepare"),
            prompt_id: Some("prompt_identity".to_owned()),
        },
        prepare_response()
    );
    check!(
        encoding,
        outcome,
        AcknowledgeBatch,
        proto::AcknowledgeBatchRequest {
            attachment_ref: attachment("acknowledge"),
            batch_id: Some("acknowledged_batch".to_owned()),
        },
        acknowledge_response()
    );
}

#[tokio::test]
async fn all_eight_methods_preserve_owner_request_and_route_response_in_both_codecs() {
    for encoding in [ContentType::Protobuf, ContentType::Json] {
        assert_matrix(encoding, BackendOutcome::Response).await;
    }
}

#[tokio::test]
async fn all_eight_methods_preserve_complete_typed_backend_faults_in_both_codecs() {
    for encoding in [ContentType::Protobuf, ContentType::Json] {
        assert_matrix(encoding, BackendOutcome::Fault).await;
    }
}
