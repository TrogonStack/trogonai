#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::{HeaderMap, Message};
use buffa::Enumeration as _;
use buffa_types::google::protobuf::Any;
use bytes::Bytes;
use futures_util::{FutureExt as _, StreamExt as _};
use grpc_nats_micro::client::{self, RequestError};
use grpc_nats_micro::constants::{HEADER_CONTENT_TYPE, HEADER_ERROR, HEADER_ERROR_CODE};
use grpc_nats_micro::server::dispatch_admitted;
use grpc_nats_micro::status_codec::{self, ReplyError};
use grpc_nats_micro::{
    AdmittedEndpoint, ContentType, DiscoveryMetadata, DiscoveryMetadataInput, EndpointAdmission, EndpointHandler,
    MethodName, MethodNameInput, NamedAdmittedEndpoint, RequestMetadata, ServeError, ServerSubjectPattern,
    ServiceBinding, ServiceDescriptor, ServiceFault, ServiceName, ServiceNameInput, ServiceVersion,
    ServiceVersionInput, SubjectPrefix, SubjectPrefixInput,
};
use trogon_nats::test_support::CoreTestServer;
use trogon_nats::{AdvancedMockNatsClient, NatsConfig, NatsTls};
use trogonai_proto::google::rpc::{Code, ErrorInfo, Status};
use trogonai_proto::grpc_nats_micro::v1::{FailRequest, FailResponse, SayRequest, SayResponse};
use trogonai_proto::nats::micro::v1alpha1::{ContentType as ProtoContentType, ServiceOptions};

const BOUND: Duration = Duration::from_secs(5);
const SAY_PATTERN: &str = "admitted.v1.*.AdmissionService.Say";
const FAIL_PATTERN: &str = "admitted.v1.*.AdmissionService.Fail";
const ALPHA_SAY: &str = "admitted.v1.alpha.AdmissionService.Say";
const BETA_SAY: &str = "admitted.v1.beta.AdmissionService.Say";
const ALPHA_FAIL: &str = "admitted.v1.alpha.AdmissionService.Fail";
const BETA_FAIL: &str = "admitted.v1.beta.AdmissionService.Fail";
const REPLY: &str = "_INBOX.admitted.reply";
const DETAIL_REASON: &str = "ADMITTED_FAULT";
const DETAIL_DOMAIN: &str = "grpc-nats-micro.admitted-contract";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Admission {
        subject: String,
        reply: Option<String>,
    },
    Handler {
        subject: String,
        content_type: ContentType,
        payload: Vec<u8>,
    },
}

#[derive(Clone, Default)]
struct Trace(Arc<Mutex<Vec<Call>>>);

impl Trace {
    fn push(&self, call: Call) {
        self.0.lock().expect("trace lock").push(call);
    }

    fn calls(&self) -> Vec<Call> {
        self.0.lock().expect("trace lock").clone()
    }
}

#[derive(Clone, Copy)]
enum ReplyMode {
    Say,
    Fail,
}

struct ScopedAdmission {
    subjects: &'static [&'static str],
    mode: ReplyMode,
    trace: Trace,
}

impl EndpointAdmission for ScopedAdmission {
    fn admit(&self, metadata: RequestMetadata<'_>) -> Option<Box<dyn EndpointHandler + '_>> {
        self.trace.push(Call::Admission {
            subject: metadata.subject.to_owned(),
            reply: metadata.reply.map(str::to_owned),
        });
        if !self.subjects.contains(&metadata.subject)
            || metadata.reply.is_some_and(|reply| !reply.starts_with("_INBOX."))
        {
            return None;
        }
        Some(Box::new(AdmittedHandler {
            subject: metadata.subject.to_owned(),
            mode: self.mode,
            trace: self.trace.clone(),
        }))
    }
}

struct AdmittedHandler {
    subject: String,
    mode: ReplyMode,
    trace: Trace,
}

impl EndpointHandler for AdmittedHandler {
    fn handle<'a>(
        &'a self,
        request_bytes: &'a [u8],
        content_type: ContentType,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ServiceFault>> + Send + 'a>> {
        Box::pin(async move {
            self.trace.push(Call::Handler {
                subject: self.subject.clone(),
                content_type,
                payload: request_bytes.to_vec(),
            });
            match self.mode {
                ReplyMode::Say => {
                    let request: SayRequest = content_type
                        .decode(request_bytes)
                        .map_err(|error| ServiceFault::invalid_argument(error.to_string()))?;
                    content_type
                        .encode(&SayResponse {
                            message: request.message,
                        })
                        .map_err(|error| ServiceFault::internal(error.to_string()))
                }
                ReplyMode::Fail => {
                    let request: FailRequest = content_type
                        .decode(request_bytes)
                        .map_err(|error| ServiceFault::invalid_argument(error.to_string()))?;
                    let code = request.code.and_then(|code| code.as_known()).unwrap_or(Code::UNKNOWN);
                    let detail = ErrorInfo {
                        reason: DETAIL_REASON.to_owned(),
                        domain: DETAIL_DOMAIN.to_owned(),
                        ..Default::default()
                    };
                    Err(ServiceFault::new(Status {
                        code: code.to_i32(),
                        message: request.message.unwrap_or_default(),
                        details: vec![Any::pack(&detail, ErrorInfo::TYPE_URL)],
                    })
                    .expect("fixture supplies a service error code"))
                }
            }
        })
    }
}

fn admission(mode: ReplyMode, trace: Trace) -> ScopedAdmission {
    ScopedAdmission {
        subjects: match mode {
            ReplyMode::Say => &[ALPHA_SAY, BETA_SAY],
            ReplyMode::Fail => &[ALPHA_FAIL, BETA_FAIL],
        },
        mode,
        trace,
    }
}

fn binding(caller: &str) -> ServiceBinding {
    ServiceBinding::new(
        ServiceName::from_input(&ServiceNameInput::new("AdmissionService")).expect("service name"),
        ServiceVersion::from_input(&ServiceVersionInput::new("1.0.0")).expect("service version"),
        SubjectPrefix::from_input(&SubjectPrefixInput::new(format!("admitted.v1.{caller}"))).expect("subject prefix"),
    )
    .with_method(
        MethodName::from_input(&MethodNameInput::new("Say")).expect("Say method"),
        DiscoveryMetadata::default(),
    )
    .expect("Say endpoint")
    .with_method(
        MethodName::from_input(&MethodNameInput::new("Fail")).expect("Fail method"),
        DiscoveryMetadata::default(),
    )
    .expect("Fail endpoint")
}

fn request(mode: ReplyMode, encoding: ContentType, text: &str) -> Vec<u8> {
    match mode {
        ReplyMode::Say => encoding
            .encode(&SayRequest {
                message: Some(text.to_owned()),
            })
            .expect("encode Say request"),
        ReplyMode::Fail => encoding
            .encode(&FailRequest {
                code: Some(Code::RESOURCE_EXHAUSTED.into()),
                message: Some(text.to_owned()),
            })
            .expect("encode Fail request"),
    }
}

fn message(mode: ReplyMode, encoding: ContentType, text: &str) -> Message {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_CONTENT_TYPE, encoding.header_value());
    let payload = request(mode, encoding, text);
    Message {
        subject: match mode {
            ReplyMode::Say => ALPHA_SAY,
            ReplyMode::Fail => ALPHA_FAIL,
        }
        .into(),
        reply: Some(REPLY.into()),
        length: payload.len(),
        payload: payload.into(),
        headers: Some(headers),
        status: None,
        description: None,
    }
}

fn published_reply(client: &AdvancedMockNatsClient) -> (HeaderMap, Bytes) {
    assert_eq!(client.published_messages(), vec![REPLY.to_owned()]);
    let headers = client.published_headers();
    let payloads = client.published_payloads();
    assert_eq!(headers.len(), 1);
    assert_eq!(payloads.len(), 1);
    (
        headers.into_iter().next().expect("one reply header map"),
        payloads.into_iter().next().expect("one reply payload"),
    )
}

fn assert_fault(error: grpc_nats_micro::ServiceError, text: &str) {
    assert_eq!(error.code().code(), Code::RESOURCE_EXHAUSTED);
    assert_eq!(error.status().code, Code::RESOURCE_EXHAUSTED.to_i32());
    assert_eq!(error.message(), text);
    assert_eq!(error.status().details.len(), 1);
    let detail = error.status().details[0]
        .unpack_if::<ErrorInfo>(ErrorInfo::TYPE_URL)
        .expect("decode complete error detail")
        .expect("ErrorInfo detail");
    assert_eq!(detail.reason, DETAIL_REASON);
    assert_eq!(detail.domain, DETAIL_DOMAIN);
}

fn assert_reply(headers: &HeaderMap, payload: &[u8], mode: ReplyMode, encoding: ContentType, text: &str) {
    assert_eq!(
        headers.get(HEADER_CONTENT_TYPE).expect("reply encoding").as_str(),
        encoding.header_value()
    );
    match mode {
        ReplyMode::Say => {
            assert!(headers.get(HEADER_ERROR_CODE).is_none());
            assert!(headers.get(HEADER_ERROR).is_none());
            let reply: SayResponse =
                status_codec::decode_reply(Some(headers), payload, encoding).expect("typed successful reply");
            assert_eq!(reply.message.as_deref(), Some(text));
        }
        ReplyMode::Fail => {
            assert_eq!(
                headers.get(HEADER_ERROR_CODE).expect("error code").as_str(),
                Code::RESOURCE_EXHAUSTED.to_i32().to_string()
            );
            assert_eq!(headers.get(HEADER_ERROR).expect("error message").as_str(), text);
            let error = status_codec::decode_reply::<FailResponse>(Some(headers), payload, encoding)
                .expect_err("typed service fault");
            match error {
                ReplyError::Service(error) => assert_fault(error, text),
                other => panic!("expected complete service fault, got {other:?}"),
            }
        }
    }
}

fn assert_dispatch_trace(trace: &Trace, message: &Message, encoding: ContentType) {
    assert_eq!(
        trace.calls(),
        vec![
            Call::Admission {
                subject: message.subject.to_string(),
                reply: message.reply.as_ref().map(ToString::to_string),
            },
            Call::Handler {
                subject: message.subject.to_string(),
                content_type: encoding,
                payload: message.payload.to_vec(),
            },
        ]
    );
}

#[tokio::test]
async fn admitted_dispatch_delivers_typed_success_and_complete_fault_in_both_encodings() {
    for mode in [ReplyMode::Say, ReplyMode::Fail] {
        for encoding in [ContentType::Protobuf, ContentType::Json] {
            let client = AdvancedMockNatsClient::new();
            let trace = Trace::default();
            let admission = admission(mode, trace.clone());
            let message = message(mode, encoding, "admitted request");
            dispatch_admitted(&client, &message, &ServiceOptions::default(), &admission).await;
            assert_dispatch_trace(&trace, &message, encoding);
            let (headers, body) = published_reply(&client);
            assert_reply(&headers, &body, mode, encoding, "admitted request");
        }
    }
}

#[tokio::test]
async fn admitted_unsupported_encoding_returns_a_readable_fault_without_invoking_the_handler() {
    for (requested, policy, reply_encoding) in [
        ("application/xml", ServiceOptions::default(), ContentType::Protobuf),
        (
            ContentType::Json.header_value(),
            ServiceOptions {
                content_type: ProtoContentType::CONTENT_TYPE_PROTOBUF.into(),
                ..Default::default()
            },
            ContentType::Json,
        ),
    ] {
        let client = AdvancedMockNatsClient::new();
        let trace = Trace::default();
        let admission = admission(ReplyMode::Say, trace.clone());
        let mut message = message(ReplyMode::Say, ContentType::Protobuf, "unused");
        message
            .headers
            .as_mut()
            .expect("request headers")
            .insert(HEADER_CONTENT_TYPE, requested);
        message.payload = vec![255].into();
        message.length = 1;
        dispatch_admitted(&client, &message, &policy, &admission).await;
        assert_eq!(
            trace.calls(),
            vec![Call::Admission {
                subject: ALPHA_SAY.to_owned(),
                reply: Some(REPLY.to_owned()),
            }]
        );
        let (headers, payload) = published_reply(&client);
        assert_eq!(
            headers.get(HEADER_CONTENT_TYPE).expect("rejection encoding").as_str(),
            reply_encoding.header_value()
        );
        let error = status_codec::decode_reply::<SayResponse>(Some(&headers), &payload, reply_encoding)
            .expect_err("unsupported encoding is a typed service fault");
        match error {
            ReplyError::Service(error) => {
                assert_eq!(error.code().code(), Code::INVALID_ARGUMENT);
                assert_eq!(error.status().code, Code::INVALID_ARGUMENT.to_i32());
                assert_eq!(error.message(), "unsupported request encoding");
                assert!(error.status().details.is_empty());
            }
            other => panic!("encoding rejection was not readable: {other:?}"),
        }
    }
}

#[tokio::test]
async fn admitted_metadata_without_reply_never_runs_the_handler_or_publishes() {
    let client = AdvancedMockNatsClient::new();
    let trace = Trace::default();
    let admission = admission(ReplyMode::Say, trace.clone());
    let mut message = message(ReplyMode::Say, ContentType::Protobuf, "not executed");
    message.reply = None;
    dispatch_admitted(&client, &message, &ServiceOptions::default(), &admission).await;
    assert!(trace.calls().is_empty());
    assert!(client.published_messages().is_empty());
    assert!(client.published_payloads().is_empty());
    assert!(client.published_headers().is_empty());
}

#[tokio::test]
async fn failed_admitted_reply_is_not_delivered_or_retried_and_the_next_request_is_independent() {
    for mode in [ReplyMode::Say, ReplyMode::Fail] {
        for encoding in [ContentType::Protobuf, ContentType::Json] {
            let client = AdvancedMockNatsClient::new();
            let trace = Trace::default();
            let admission = admission(mode, trace.clone());
            let first = message(mode, encoding, "first response is lost");
            client.fail_next_publish();
            dispatch_admitted(&client, &first, &ServiceOptions::default(), &admission).await;
            assert_dispatch_trace(&trace, &first, encoding);
            assert!(client.published_messages().is_empty());
            assert!(client.published_payloads().is_empty());
            assert!(client.published_headers().is_empty());

            let mut expected = trace.calls();
            let next = message(mode, encoding, "second response is independent");
            dispatch_admitted(&client, &next, &ServiceOptions::default(), &admission).await;
            expected.extend([
                Call::Admission {
                    subject: next.subject.to_string(),
                    reply: Some(REPLY.to_owned()),
                },
                Call::Handler {
                    subject: next.subject.to_string(),
                    content_type: encoding,
                    payload: next.payload.to_vec(),
                },
            ]);
            assert_eq!(trace.calls(), expected);
            let (headers, body) = published_reply(&client);
            assert_reply(&headers, &body, mode, encoding, "second response is independent");
        }
    }
}

async fn subscription_barrier(client: &async_nats::Client) {
    tokio::time::timeout(BOUND, async {
        let subject = client.new_inbox();
        let mut incoming = client.subscribe(subject.clone()).await.expect("barrier subscription");
        client
            .publish(subject, Bytes::from_static(b"ready"))
            .await
            .expect("barrier publication");
        let message = incoming.next().await.expect("broker observed registration barrier");
        assert_eq!(message.payload.as_ref(), b"ready");
        incoming.unsubscribe().await.expect("barrier unsubscribe");
    })
    .await
    .expect("broker registration barrier stayed bounded");
}

#[derive(Clone, Copy)]
enum AdmissionRegistration {
    Legacy,
    Named,
}

#[derive(Clone, Copy)]
enum DiscoveryMetadataMode {
    Empty,
    Populated,
}

fn discovery_metadata(key: &str, value: &str) -> DiscoveryMetadata {
    DiscoveryMetadata::from_input(&DiscoveryMetadataInput::new([(key, value)])).expect("valid discovery metadata")
}

fn populated_binding(caller: &str) -> ServiceBinding {
    ServiceBinding::new(
        ServiceName::from_input(&ServiceNameInput::new("AdmissionService")).expect("service name"),
        ServiceVersion::from_input(&ServiceVersionInput::new("1.0.0")).expect("service version"),
        SubjectPrefix::from_input(&SubjectPrefixInput::new(format!("admitted.v1.{caller}"))).expect("subject prefix"),
    )
    .with_description("Admission contract")
    .with_metadata(discovery_metadata("contract", "admitted"))
    .with_method(
        MethodName::from_input(&MethodNameInput::new("Say")).expect("Say method"),
        discovery_metadata("operation", "say"),
    )
    .expect("Say endpoint")
    .with_method(
        MethodName::from_input(&MethodNameInput::new("Fail")).expect("Fail method"),
        discovery_metadata("operation", "fail"),
    )
    .expect("Fail endpoint")
}

fn endpoint_metadata(mode: DiscoveryMetadataMode, operation: &str) -> DiscoveryMetadata {
    match mode {
        DiscoveryMetadataMode::Empty => DiscoveryMetadata::default(),
        DiscoveryMetadataMode::Populated => discovery_metadata("operation", operation),
    }
}

async fn assert_admitted_service_contract(registration: AdmissionRegistration, metadata_mode: DiscoveryMetadataMode) {
    let server = CoreTestServer::start().await;
    let config = NatsConfig::from_url(server.address()).with_tls(NatsTls::Disabled);
    let transport = trogon_nats::connect(&config, BOUND)
        .await
        .expect("connect to isolated broker");
    let declared = match metadata_mode {
        DiscoveryMetadataMode::Empty => binding("declaration"),
        DiscoveryMetadataMode::Populated => populated_binding("declaration"),
    };
    assert_eq!(declared.name().as_str(), "AdmissionService");
    assert_eq!(declared.version().as_str(), "1.0.0");
    assert_eq!(declared.subject_prefix().as_str(), "admitted.v1.declaration");
    match metadata_mode {
        DiscoveryMetadataMode::Empty => {
            assert_eq!(declared.description(), None);
            assert!(declared.metadata().is_empty());
            assert!(
                declared
                    .endpoints()
                    .iter()
                    .all(|endpoint| endpoint.metadata().is_empty())
            );
        }
        DiscoveryMetadataMode::Populated => {
            assert_eq!(declared.description(), Some("Admission contract"));
            assert_eq!(
                declared.metadata().entries().get("contract").map(String::as_str),
                Some("admitted")
            );
        }
    }
    let trace = Trace::default();
    let say_pattern = ServerSubjectPattern::new(SAY_PATTERN).expect("valid Say wildcard");
    let fail_pattern = ServerSubjectPattern::new(FAIL_PATTERN).expect("valid Fail wildcard");
    assert_eq!(say_pattern.as_str(), SAY_PATTERN);
    assert_eq!(fail_pattern.as_str(), FAIL_PATTERN);
    assert_ne!(say_pattern.as_str(), declared.endpoints()[0].subject().as_str());

    let mismatch = tokio::time::timeout(
        BOUND,
        grpc_nats_micro::serve_admitted(
            &transport,
            &declared,
            ServiceOptions::default(),
            vec![AdmittedEndpoint {
                subject: say_pattern.clone(),
                admission: Box::new(admission(ReplyMode::Say, trace.clone())),
            }],
        ),
    )
    .await
    .expect("mismatch rejection stayed bounded")
    .expect_err("a partial admitted handler list must not register a service");
    assert!(matches!(
        mismatch,
        ServeError::HandlerCount {
            endpoints: 2,
            handlers: 1
        }
    ));
    assert!(trace.calls().is_empty());

    let service = tokio::time::timeout(BOUND, async {
        match registration {
            AdmissionRegistration::Legacy => {
                grpc_nats_micro::serve_admitted(
                    &transport,
                    &declared,
                    ServiceOptions::default(),
                    vec![
                        AdmittedEndpoint {
                            subject: say_pattern,
                            admission: Box::new(admission(ReplyMode::Say, trace.clone())),
                        },
                        AdmittedEndpoint {
                            subject: fail_pattern,
                            admission: Box::new(admission(ReplyMode::Fail, trace.clone())),
                        },
                    ],
                )
                .await
            }
            AdmissionRegistration::Named => {
                let descriptor = ServiceDescriptor::new(
                    ServiceName::from_input(&ServiceNameInput::new("AdmissionService")).expect("service name"),
                    ServiceVersion::from_input(&ServiceVersionInput::new("1.0.0")).expect("service version"),
                );
                let descriptor = match metadata_mode {
                    DiscoveryMetadataMode::Empty => descriptor,
                    DiscoveryMetadataMode::Populated => descriptor
                        .with_description("Admission contract")
                        .with_metadata(discovery_metadata("contract", "admitted")),
                };
                grpc_nats_micro::serve_named_admitted(
                    &transport,
                    &descriptor,
                    ServiceOptions::default(),
                    vec![
                        NamedAdmittedEndpoint {
                            method_name: MethodName::from_input(&MethodNameInput::new("Say")).expect("Say method"),
                            metadata: endpoint_metadata(metadata_mode, "say"),
                            subject: say_pattern,
                            admission: Box::new(admission(ReplyMode::Say, trace.clone())),
                        },
                        NamedAdmittedEndpoint {
                            method_name: MethodName::from_input(&MethodNameInput::new("Fail")).expect("Fail method"),
                            metadata: endpoint_metadata(metadata_mode, "fail"),
                            subject: fail_pattern,
                            admission: Box::new(admission(ReplyMode::Fail, trace.clone())),
                        },
                    ],
                )
                .await
            }
        }
    })
    .await
    .expect("registration stayed bounded")
    .expect("register admitted service");
    subscription_barrier(&transport).await;
    let info = tokio::time::timeout(BOUND, transport.request("$SRV.INFO.AdmissionService", Bytes::new()))
        .await
        .expect("discovery query stayed bounded")
        .expect("discovery response");
    let info: serde_json::Value = serde_json::from_slice(&info.payload).expect("discovery JSON");
    assert_eq!(info["name"], "AdmissionService");
    assert_eq!(info["version"], "1.0.0");
    match metadata_mode {
        DiscoveryMetadataMode::Empty => assert_eq!(info["metadata"], serde_json::json!({})),
        DiscoveryMetadataMode::Populated => {
            assert_eq!(info["description"], "Admission contract");
            assert_eq!(info["metadata"]["contract"], "admitted");
        }
    }
    let endpoints = info["endpoints"].as_array().expect("endpoint discovery");
    assert_eq!(endpoints.len(), 2);
    for (name, pattern, operation) in [("Say", SAY_PATTERN, "say"), ("Fail", FAIL_PATTERN, "fail")] {
        let endpoint = endpoints
            .iter()
            .find(|endpoint| endpoint["name"] == name)
            .expect("named endpoint");
        assert_eq!(endpoint["subject"], pattern);
        assert_eq!(endpoint["queue_group"], "q");
        match metadata_mode {
            DiscoveryMetadataMode::Empty => assert_eq!(endpoint["metadata"], serde_json::json!({})),
            DiscoveryMetadataMode::Populated => assert_eq!(endpoint["metadata"]["operation"], operation),
        }
    }
    transport
        .publish(ALPHA_SAY, Bytes::from_static(&[255]))
        .await
        .expect("publish no-reply request that cannot run admission or decode");

    let denied_subject = "admitted.v1.gamma.AdmissionService.Say";
    let denied_reply = transport.new_inbox();
    let mut denied_replies = transport
        .subscribe(denied_reply.clone())
        .await
        .expect("denied reply probe");
    let mut denied_headers = HeaderMap::new();
    denied_headers.insert(HEADER_CONTENT_TYPE, "application/unsafe");
    transport
        .publish_with_reply_and_headers(
            denied_subject,
            denied_reply.clone(),
            denied_headers,
            Bytes::from_static(&[255]),
        )
        .await
        .expect("publish rejected caller probe");
    let mut expected = Vec::new();
    for caller in ["alpha", "beta"] {
        let concrete = binding(caller);
        for encoding in [ContentType::Protobuf, ContentType::Json] {
            let text = format!("{caller} admitted");
            let say = SayRequest {
                message: Some(text.clone()),
            };
            let response: SayResponse = client::request(&transport, &concrete.endpoints()[0], encoding, &say, BOUND)
                .await
                .expect("concrete subject reached wildcard Say handler");
            assert_eq!(response.message.as_deref(), Some(text.as_str()));
            expected.push((
                concrete.endpoints()[0].subject().as_str().to_owned(),
                encoding,
                request(ReplyMode::Say, encoding, &text),
            ));

            let fail = FailRequest {
                code: Some(Code::RESOURCE_EXHAUSTED.into()),
                message: Some(text.clone()),
            };
            let error = client::request::<_, FailRequest, FailResponse>(
                &transport,
                &concrete.endpoints()[1],
                encoding,
                &fail,
                BOUND,
            )
            .await
            .expect_err("concrete subject reached wildcard Fail handler");
            match error {
                RequestError::Reply(ReplyError::Service(error)) => assert_fault(error, &text),
                other => panic!("wildcard handler did not return a typed fault: {other:?}"),
            }
            expected.push((
                concrete.endpoints()[1].subject().as_str().to_owned(),
                encoding,
                request(ReplyMode::Fail, encoding, &text),
            ));
        }
    }
    let observed = trace.calls();
    assert_eq!(observed.len(), expected.len() * 2 + 1);
    assert_eq!(
        observed[0],
        Call::Admission {
            subject: denied_subject.to_owned(),
            reply: Some(denied_reply.to_string()),
        }
    );
    assert!(
        denied_replies.next().now_or_never().is_none(),
        "denied traffic must not receive even an encoding-error reply"
    );
    denied_replies.unsubscribe().await.expect("close denied reply probe");
    for (pair, (subject, content_type, payload)) in observed[1..].chunks_exact(2).zip(expected) {
        match &pair[0] {
            Call::Admission {
                subject: admitted_subject,
                reply,
            } => {
                assert_eq!(admitted_subject, &subject);
                assert!(reply.as_deref().is_some_and(|reply| reply.starts_with("_INBOX.")));
            }
            other => panic!("handler ran before admission: {other:?}"),
        }
        assert_eq!(
            pair[1],
            Call::Handler {
                subject,
                content_type,
                payload
            }
        );
    }
    let stats = tokio::time::timeout(BOUND, transport.request("$SRV.STATS.AdmissionService", Bytes::new()))
        .await
        .expect("micro statistics query stayed bounded")
        .expect("micro statistics response");
    let stats: serde_json::Value = serde_json::from_slice(&stats.payload).expect("micro statistics JSON");
    let say = stats["endpoints"]
        .as_array()
        .expect("endpoint statistics")
        .iter()
        .find(|endpoint| endpoint["name"] == "Say")
        .expect("admitted Say endpoint statistics");
    assert_eq!(
        say["num_requests"], 4,
        "all four successful admitted calls must remain observable"
    );
    assert!(say["processing_time"].as_u64().is_some_and(|elapsed| elapsed > 0));
    tokio::time::timeout(BOUND, service.stop())
        .await
        .expect("service stop stayed bounded")
        .expect("stop admitted service");
}

#[tokio::test]
async fn admitted_service_registers_wildcards_and_serves_metadata_scoped_typed_round_trips() {
    assert_admitted_service_contract(AdmissionRegistration::Legacy, DiscoveryMetadataMode::Empty).await;
}

#[tokio::test]
async fn named_admitted_service_preserves_discovery_scoped_dispatch_faults_and_stats() {
    assert_admitted_service_contract(AdmissionRegistration::Named, DiscoveryMetadataMode::Empty).await;
}

#[test]
fn service_binding_debug_preserves_the_existing_public_diagnostic_shape() {
    let binding = ServiceBinding::new(
        ServiceName::from_input(&ServiceNameInput::new("AdmissionService")).expect("service name"),
        ServiceVersion::from_input(&ServiceVersionInput::new("1.0.0")).expect("service version"),
        SubjectPrefix::from_input(&SubjectPrefixInput::new("admitted.v1")).expect("subject prefix"),
    );
    assert_eq!(
        format!("{binding:?}"),
        r#"ServiceBinding { name: ServiceName(NatsToken("AdmissionService")), version: ServiceVersion("1.0.0"), description: None, metadata: DiscoveryMetadata({}), subject_prefix: SubjectPrefix(DottedNatsToken("admitted.v1")), endpoints: [] }"#
    );
}

#[tokio::test]
async fn admitted_service_preserves_populated_service_and_endpoint_metadata() {
    assert_admitted_service_contract(AdmissionRegistration::Legacy, DiscoveryMetadataMode::Populated).await;
}

#[tokio::test]
async fn named_admitted_service_preserves_populated_service_and_endpoint_metadata() {
    assert_admitted_service_contract(AdmissionRegistration::Named, DiscoveryMetadataMode::Populated).await;
}
