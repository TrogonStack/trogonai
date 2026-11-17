use std::time::Duration;

use async_nats::HeaderMap;
use buffa::MessageField;
use bytes::Bytes;
use grpc_nats_micro::client::{RequestError, request as call};
use grpc_nats_micro::constants::{CONTENT_TYPE_PROTOBUF, HEADER_CONTENT_TYPE, HEADER_ERROR_CODE};
use grpc_nats_micro::{
    ContentType, DiscoveryMetadata, EndpointBinding, MethodName, MethodNameInput, ReplyError, ServiceErrorCode,
    ServiceName, ServiceNameInput, SubjectPrefix, SubjectPrefixInput,
};
use nkeys::XKey;
use trogon_nats::RequestClient;
use trogonai_proto::secrets::exchange::v1alpha1::resolve_for_release_response::Result as Outcome;
use trogonai_proto::secrets::exchange::v1alpha1::{
    RequesterXkey, ResolveFailureCategory as Category, ResolveForReleaseRequest, ResolveForReleaseResponse,
};

use super::FakeSecretsService;
use crate::fixtures::{BUNDLE_GENERATION, now, request, secret, tenant};
use crate::{FakeOutcome, FakeRequestError};

const SUBJECT: &str = "fake.secrets.v1.service.billing-runner.SecretExchangeService.ResolveForRelease";

fn endpoint() -> EndpointBinding {
    EndpointBinding::new(
        &SubjectPrefix::from_input(&SubjectPrefixInput::new("fake.secrets.v1.service.billing-runner")).unwrap(),
        &ServiceName::from_input(&ServiceNameInput::new("SecretExchangeService")).unwrap(),
        MethodName::from_input(&MethodNameInput::new("ResolveForRelease")).unwrap(),
        DiscoveryMetadata::default(),
    )
    .unwrap()
}

fn protobuf() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_CONTENT_TYPE, CONTENT_TYPE_PROTOBUF);
    headers
}

fn asking_with(requester_xkey: &str) -> ResolveForReleaseRequest {
    let mut asked = request(&tenant()).to_wire(String::new());
    asked.requester_xkey = MessageField::some(RequesterXkey {
        public_key: requester_xkey.to_owned(),
    });
    asked
}

#[tokio::test]
async fn refuses_a_payload_that_does_not_decode_on_the_error_channel() {
    let fake = FakeSecretsService::new();
    fake.script(FakeOutcome::release(
        secret(),
        now() + Duration::from_secs(60),
        BUNDLE_GENERATION,
    ));

    let reply = fake
        .request_with_headers(SUBJECT, protobuf(), Bytes::from_static(&[0xff, 0xff, 0xff]))
        .await
        .unwrap();

    let code = reply
        .headers
        .unwrap()
        .get(HEADER_ERROR_CODE)
        .map(|value| value.as_str().to_owned());
    assert_eq!(code, Some(ServiceErrorCode::INVALID_ARGUMENT.to_i32().to_string()));
    assert!(fake.received().is_empty());
}

#[tokio::test]
async fn refuses_a_requester_xkey_that_is_not_a_curve_public_key() {
    let fake = FakeSecretsService::new();
    fake.script(FakeOutcome::release(
        secret(),
        now() + Duration::from_secs(60),
        BUNDLE_GENERATION,
    ));

    let answered: ResolveForReleaseResponse = call(
        &fake,
        &endpoint(),
        ContentType::Protobuf,
        &asking_with(&XKey::new().seed().unwrap()),
        Duration::from_secs(1),
    )
    .await
    .unwrap();

    assert!(matches!(
        answered.result,
        Some(Outcome::Failure(failure)) if failure.category.as_known() == Some(Category::RESOLVE_FAILURE_CATEGORY_KEY_DENIED)
    ));
}

#[tokio::test]
async fn denies_once_its_script_runs_out() {
    let fake = FakeSecretsService::default();

    let answered: ResolveForReleaseResponse = call(
        &fake,
        &endpoint(),
        ContentType::Protobuf,
        &asking_with(&XKey::new().public_key()),
        Duration::from_secs(1),
    )
    .await
    .unwrap();

    assert!(matches!(
        answered.result,
        Some(Outcome::Failure(failure)) if failure.category.as_known() == Some(Category::RESOLVE_FAILURE_CATEGORY_DENIED)
    ));
    assert_eq!(fake.received()[0].subject(), SUBJECT);
}

#[tokio::test]
async fn answers_nothing_when_scripted_unreachable() {
    let fake = FakeSecretsService::new();
    fake.script(FakeOutcome::unreachable());

    let failed = call::<_, _, ResolveForReleaseResponse>(
        &fake,
        &endpoint(),
        ContentType::Protobuf,
        &asking_with(&XKey::new().public_key()),
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        failed,
        RequestError::Transport {
            error: FakeRequestError::Unreachable,
            ..
        }
    ));
    assert_eq!(
        FakeRequestError::Unreachable.to_string(),
        "fake secrets service is unreachable"
    );
    assert!(fake.received().is_empty());
}

#[tokio::test(start_paused = true)]
async fn answers_only_after_the_scripted_delay() {
    let fake = FakeSecretsService::new();
    fake.script(FakeOutcome::denied().after(Duration::from_secs(3)));
    let started = tokio::time::Instant::now();

    let answered = call::<_, _, ResolveForReleaseResponse>(
        &fake,
        &endpoint(),
        ContentType::Protobuf,
        &asking_with(&XKey::new().public_key()),
        Duration::from_secs(5),
    )
    .await;

    assert!(answered.is_ok());
    assert_eq!(started.elapsed(), Duration::from_secs(3));
}

#[tokio::test]
async fn takes_its_outcomes_in_the_order_they_were_scripted() {
    let fake = FakeSecretsService::new();
    fake.script(FakeOutcome::invalid_argument("fake-invalid"));
    fake.script(FakeOutcome::unavailable());

    let first = call::<_, _, ResolveForReleaseResponse>(
        &fake,
        &endpoint(),
        ContentType::Protobuf,
        &asking_with(&XKey::new().public_key()),
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();
    let second: ResolveForReleaseResponse = call(
        &fake,
        &endpoint(),
        ContentType::Protobuf,
        &asking_with(&XKey::new().public_key()),
        Duration::from_secs(1),
    )
    .await
    .unwrap();

    assert!(matches!(
        first,
        RequestError::Reply(ReplyError::Service(fault))
            if fault.code() == ServiceErrorCode::INVALID_ARGUMENT && fault.message() == "fake-invalid"
    ));
    assert!(matches!(
        second.result,
        Some(Outcome::Failure(failure)) if failure.category.as_known() == Some(Category::RESOLVE_FAILURE_CATEGORY_UNAVAILABLE)
    ));
}

#[tokio::test(start_paused = true)]
async fn records_each_request_as_it_arrives_not_as_it_is_answered() {
    let fake = FakeSecretsService::new();
    fake.script(FakeOutcome::denied().after(Duration::from_secs(3)));
    fake.script(FakeOutcome::denied());
    let binding = endpoint();
    let (slow, fast) = (XKey::new().public_key(), XKey::new().public_key());
    let (asked_slow, asked_fast) = (asking_with(&slow), asking_with(&fast));

    let mut first = std::pin::pin!(call::<_, _, ResolveForReleaseResponse>(
        &fake,
        &binding,
        ContentType::Protobuf,
        &asked_slow,
        Duration::from_secs(5),
    ));
    assert!(futures::poll!(&mut first).is_pending());
    call::<_, _, ResolveForReleaseResponse>(
        &fake,
        &binding,
        ContentType::Protobuf,
        &asked_fast,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    first.await.unwrap();

    let askers: Vec<String> = fake
        .received()
        .iter()
        .map(|received| received.request().requester_xkey.public_key.clone())
        .collect();
    assert_eq!(askers, [slow, fast]);
}
