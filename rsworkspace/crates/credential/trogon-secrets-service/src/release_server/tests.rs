use std::sync::Arc;

use buffa::Message as _;
use grpc_nats_micro::ContentType;
use grpc_nats_micro::constants::{DEFAULT_QUEUE_GROUP, HEADER_CONTENT_TYPE};
use grpc_nats_micro::status_codec::decode_reply;
use nkeys::XKey;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
use trogonai_proto::secrets::exchange::v1alpha1::resolve_for_release_response::Result as Outcome;
use trogonai_proto::secrets::exchange::v1alpha1::{
    ResolveFailure, ResolveFailureCategory, ResolveForReleaseResponse, SealedAccess,
};

use super::*;
use crate::fixtures::{release_subject, release_wire, request, sealed_release, tenant};
use crate::scripted_release::ScriptedRelease;
use crate::{RELEASE_DEADLINE, RELEASE_SUBSCRIPTION, THROTTLED_RETRY_AFTER};

const REPLY: &str = "_INBOX.fake.1";
const OTHER_REPLY: &str = "_INBOX.fake.2";

fn call(subject: &str, reply: Option<&str>, requester_xkey: &str) -> async_nats::Message {
    async_nats::Message {
        subject: subject.into(),
        reply: reply.map(Into::into),
        payload: release_wire(requester_xkey).encode_to_vec().into(),
        headers: None,
        status: None,
        description: None,
        length: 0,
    }
}

fn any_call(subject: &str, reply: Option<&str>) -> async_nats::Message {
    call(subject, reply, &XKey::new().public_key())
}

fn answered(headers: &[async_nats::HeaderMap], payloads: &[bytes::Bytes], index: usize) -> Outcome {
    decode_reply::<ResolveForReleaseResponse>(Some(&headers[index]), &payloads[index], ContentType::Protobuf)
        .unwrap()
        .result
        .unwrap()
}

fn failure(outcome: Outcome) -> ResolveFailure {
    match outcome {
        Outcome::Failure(failure) => *failure,
        Outcome::Access(_) => panic!("a call answered an access"),
    }
}

fn access(outcome: Outcome) -> SealedAccess {
    match outcome {
        Outcome::Access(access) => *access,
        Outcome::Failure(failure) => panic!("a call answered a failure: {failure:?}"),
    }
}

#[tokio::test]
async fn a_bus_that_refuses_the_queue_subscription_is_an_error() {
    let release = Arc::new(ScriptedRelease::answering(Ok(sealed_release())));
    let server = ReleaseServer::new(MockNatsClient::new(), release, AdmissionLimit::default());
    let error = server.run(CancellationToken::new()).await.unwrap_err();
    assert!(matches!(error, ReleaseServerError::Join(_)));
}

#[tokio::test]
async fn a_subscription_that_ends_before_shutdown_is_an_error() {
    let client = MockNatsClient::new();
    drop(client.inject_messages());
    let release = Arc::new(ScriptedRelease::answering(Ok(sealed_release())));
    let server = ReleaseServer::new(client, release, AdmissionLimit::default());
    let error = server.run(CancellationToken::new()).await.unwrap_err();
    assert!(matches!(error, ReleaseServerError::SubscriptionEnded));
}

#[tokio::test]
async fn a_call_without_a_reply_subject_is_dropped_unasked() {
    let client = MockNatsClient::new();
    let calls = client.inject_messages();
    calls
        .unbounded_send(any_call(&release_subject(&tenant()), None))
        .unwrap();
    drop(calls);
    let release = Arc::new(ScriptedRelease::answering(Ok(sealed_release())));
    let server = ReleaseServer::new(client.clone(), Arc::clone(&release), AdmissionLimit::default());
    server.run(CancellationToken::new()).await.unwrap_err();
    assert!(client.published_messages().is_empty());
    assert!(release.calls().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_call_beyond_the_admission_limit_is_throttled_with_its_retry_delay() {
    let client = MockNatsClient::new();
    let calls = client.inject_messages();
    let subject = release_subject(&tenant());
    calls.unbounded_send(any_call(&subject, Some(REPLY))).unwrap();
    calls.unbounded_send(any_call(&subject, Some(OTHER_REPLY))).unwrap();
    drop(calls);
    let release = Arc::new(ScriptedRelease::hanging());
    let server = ReleaseServer::new(client.clone(), Arc::clone(&release), AdmissionLimit::new(1).unwrap());
    server.run(CancellationToken::new()).await.unwrap_err();
    assert_eq!(client.published_messages(), vec![OTHER_REPLY, REPLY]);
    let (headers, payloads) = (client.published_headers(), client.published_payloads());
    let throttled = failure(answered(&headers, &payloads, 0));
    assert_eq!(
        throttled.category.as_known(),
        Some(ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_THROTTLED)
    );
    assert_eq!(
        throttled.retry_after.as_option(),
        Some(&buffa_types::google::protobuf::Duration::from(THROTTLED_RETRY_AFTER))
    );
    assert_eq!(
        failure(answered(&headers, &payloads, 1)).category.as_known(),
        Some(ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_UNAVAILABLE)
    );
    assert_eq!(release.calls().len(), 1);
}

#[tokio::test]
async fn a_reply_the_bus_refuses_does_not_stop_the_server() {
    let client = AdvancedMockNatsClient::new();
    client.fail_next_publish();
    let calls = client.inject_messages();
    let subject = release_subject(&tenant());
    calls.unbounded_send(any_call(&subject, Some(REPLY))).unwrap();
    calls.unbounded_send(any_call(&subject, Some(OTHER_REPLY))).unwrap();
    drop(calls);
    let release = Arc::new(ScriptedRelease::answering(Ok(sealed_release())));
    let server = ReleaseServer::new(client.clone(), Arc::clone(&release), AdmissionLimit::default());
    let error = server.run(CancellationToken::new()).await.unwrap_err();
    assert!(matches!(error, ReleaseServerError::SubscriptionEnded));
    assert_eq!(client.published_messages().len(), 1);
    assert_eq!(release.calls().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn shutdown_stops_taking_calls_and_answers_those_in_flight() {
    let client = MockNatsClient::new();
    let calls = client.inject_messages();
    let subject = release_subject(&tenant());
    let release = Arc::new(ScriptedRelease::hanging());
    let shutdown = CancellationToken::new();
    let started = Instant::now();
    let server = tokio::spawn(
        ReleaseServer::new(client.clone(), Arc::clone(&release), AdmissionLimit::default()).run(shutdown.clone()),
    );
    calls.unbounded_send(any_call(&subject, Some(REPLY))).unwrap();
    release.called().await;
    shutdown.cancel();
    calls.unbounded_send(any_call(&subject, Some(OTHER_REPLY))).unwrap();
    server.await.unwrap().unwrap();
    assert_eq!(started.elapsed(), RELEASE_DEADLINE);
    assert_eq!(client.published_messages(), vec![REPLY]);
    assert_eq!(release.calls().len(), 1);
}

#[tokio::test]
async fn a_release_call_is_answered_on_its_reply_subject_in_the_contract_encoding() {
    let client = MockNatsClient::new();
    let calls = client.inject_messages();
    let tenant = tenant();
    let xkey = XKey::new().public_key();
    calls
        .unbounded_send(call(&release_subject(&tenant), Some(REPLY), &xkey))
        .unwrap();
    drop(calls);
    let release = Arc::new(ScriptedRelease::answering(Ok(sealed_release())));
    let server = ReleaseServer::new(client.clone(), Arc::clone(&release), AdmissionLimit::default());
    server.run(CancellationToken::new()).await.unwrap_err();
    assert_eq!(
        client.queue_subscribed_to(),
        vec![(RELEASE_SUBSCRIPTION.to_owned(), DEFAULT_QUEUE_GROUP.to_owned())]
    );
    assert_eq!(client.published_messages(), vec![REPLY]);
    let headers = client.published_headers();
    assert_eq!(
        headers[0].get(HEADER_CONTENT_TYPE).map(|value| value.as_str()),
        Some(ContentType::Protobuf.header_value())
    );
    let access = access(answered(&headers, &client.published_payloads(), 0));
    assert_eq!(access.xkey_sealed_value, sealed_release().sealed_value().as_bytes());
    assert_eq!(release.calls(), vec![(request(&tenant), xkey)]);
}
