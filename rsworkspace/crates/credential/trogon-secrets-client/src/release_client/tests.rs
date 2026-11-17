use std::time::{Duration, SystemTime};

use async_nats::HeaderMap;
use buffa::Message;
use bytes::Bytes;
use grpc_nats_micro::constants::{CONTENT_TYPE_PROTOBUF, HEADER_CONTENT_TYPE};
use grpc_nats_micro::status_codec::encode_reply;
use grpc_nats_micro::{ContentType, Outcome, ReplyError, ServiceErrorCode, ServiceFault};
use nkeys::XKey;
use nkeys::error::ErrorKind;
use trogon_nats::mocks::AdvancedMockNatsClient;
use trogon_std::time::FixedEpochClock;
use trogonai_proto::secrets::exchange::v1alpha1::release_location::Value;
use trogonai_proto::secrets::exchange::v1alpha1::{
    ResolveFailure, ResolveFailureCategory as Category, ResolveForReleaseResponse,
};

use super::ReleaseClient;
use crate::fixtures::{
    BUNDLE_GENERATION, CORRELATION_ID, CREDENTIAL_ID, HEADER, HOST, OWNER, PORT, VALUE, clock, now, pinned, request,
    runtime_service, secret, tenant,
};
use crate::{
    FakeOutcome, FakeSecretsService, InvalidReleaseResponseError, RELEASE_HOLD_BOUND, ReleaseClientConfig,
    ReleaseError, ReleaseTimeout, TenantAccount, UnsealError,
};

fn subject(tenant: &TenantAccount) -> String {
    format!("{tenant}.secrets.v1.service.billing-runner.SecretExchangeService.ResolveForRelease")
}

fn in_a_minute() -> SystemTime {
    now() + Duration::from_secs(60)
}

fn against_fake(fake: &FakeSecretsService) -> ReleaseClient<FakeSecretsService, FixedEpochClock> {
    ReleaseClient::new(
        fake.clone(),
        clock(),
        ReleaseClientConfig::new(runtime_service(), fake.service_xkey()),
    )
}

fn against_mock(mock: &AdvancedMockNatsClient) -> ReleaseClient<AdvancedMockNatsClient, FixedEpochClock> {
    ReleaseClient::new(
        mock.clone(),
        clock(),
        ReleaseClientConfig::new(runtime_service(), pinned(&XKey::new())),
    )
}

fn answer_on_mock(mock: &AdvancedMockNatsClient, tenant: &TenantAccount, outcome: Outcome) {
    let mut reply = encode_reply(outcome, ContentType::Protobuf).unwrap();
    reply.headers.insert(HEADER_CONTENT_TYPE, CONTENT_TYPE_PROTOBUF);
    mock.set_response_wire(&subject(tenant), reply.headers, reply.body);
}

fn respond_on_mock(mock: &AdvancedMockNatsClient, tenant: &TenantAccount, response: &ResolveForReleaseResponse) {
    answer_on_mock(mock, tenant, Outcome::Success(Bytes::from(response.encode_to_vec())));
}

async fn released_after(outcome: FakeOutcome) -> Result<crate::ReleasedValue, ReleaseError> {
    let fake = FakeSecretsService::new();
    fake.script(outcome);
    against_fake(&fake).release(&request(&tenant())).await
}

#[tokio::test(start_paused = true)]
async fn times_out_when_no_answer_arrives_within_the_timeout() {
    let fake = FakeSecretsService::new();
    fake.script(FakeOutcome::release(secret(), in_a_minute(), BUNDLE_GENERATION).after(Duration::from_secs(3)));

    let refused = against_fake(&fake).release(&request(&tenant())).await.unwrap_err();

    assert!(matches!(refused, ReleaseError::Timeout));
}

#[tokio::test(start_paused = true)]
async fn waits_as_long_as_it_was_configured_to() {
    let fake = FakeSecretsService::new();
    fake.script(FakeOutcome::release(secret(), in_a_minute(), BUNDLE_GENERATION).after(Duration::from_secs(3)));
    let config = ReleaseClientConfig::new(runtime_service(), fake.service_xkey())
        .with_timeout(ReleaseTimeout::new(Duration::from_secs(4)).unwrap());
    let client = ReleaseClient::new(fake.clone(), clock(), config);

    let released = client.release(&request(&tenant())).await.unwrap();

    assert_eq!(released.value().as_str(), VALUE);
    assert_eq!(client.config().timeout().get(), Duration::from_secs(4));
}

#[tokio::test]
async fn fails_when_the_bus_is_unreachable() {
    let refused = released_after(FakeOutcome::unreachable()).await.unwrap_err();

    assert!(matches!(refused, ReleaseError::Request(_)));
}

#[tokio::test]
async fn reads_a_refused_request_on_the_error_channel() {
    let refused = released_after(FakeOutcome::invalid_argument("fake-invalid"))
        .await
        .unwrap_err();

    assert!(
        matches!(refused, ReleaseError::InvalidArgument(fault) if fault.code() == ServiceErrorCode::INVALID_ARGUMENT)
    );
}

#[tokio::test]
async fn reads_any_other_error_channel_code_as_a_service_fault() {
    let mock = AdvancedMockNatsClient::new();
    let tenant = tenant();
    answer_on_mock(&mock, &tenant, Outcome::Error(ServiceFault::internal("fake-internal")));

    let refused = against_mock(&mock).release(&request(&tenant)).await.unwrap_err();

    assert!(matches!(refused, ReleaseError::ServiceFault(fault) if fault.code() == ServiceErrorCode::INTERNAL));
}

#[tokio::test]
async fn refuses_a_reply_that_does_not_decode() {
    let mock = AdvancedMockNatsClient::new();
    let tenant = tenant();
    answer_on_mock(
        &mock,
        &tenant,
        Outcome::Success(Bytes::from_static(&[0xff, 0xff, 0xff])),
    );

    let refused = against_mock(&mock).release(&request(&tenant)).await.unwrap_err();

    let ReleaseError::InvalidResponse(InvalidReleaseResponseError::Reply(reply)) = refused else {
        panic!("a reply that does not decode is an invalid response");
    };
    assert!(matches!(*reply, ReplyError::Decode(_)));
}

#[tokio::test]
async fn refuses_a_reply_with_no_result() {
    let mock = AdvancedMockNatsClient::new();
    let tenant = tenant();
    respond_on_mock(&mock, &tenant, &ResolveForReleaseResponse::default());

    let refused = against_mock(&mock).release(&request(&tenant)).await.unwrap_err();

    assert!(matches!(
        refused,
        ReleaseError::InvalidResponse(InvalidReleaseResponseError::MissingResult)
    ));
}

#[tokio::test]
async fn refuses_a_failure_the_contract_does_not_allow() {
    let mock = AdvancedMockNatsClient::new();
    let tenant = tenant();
    let response = ResolveForReleaseResponse {
        result: ResolveFailure {
            category: Category::RESOLVE_FAILURE_CATEGORY_THROTTLED.into(),
            retry_after: None.into(),
        }
        .into(),
    };
    respond_on_mock(&mock, &tenant, &response);

    let refused = against_mock(&mock).release(&request(&tenant)).await.unwrap_err();

    assert!(matches!(
        refused,
        ReleaseError::InvalidResponse(InvalidReleaseResponseError::ThrottledWithoutRetryAfter)
    ));
}

#[tokio::test]
async fn reads_each_refusal_the_service_answers() {
    assert!(matches!(
        released_after(FakeOutcome::denied()).await.unwrap_err(),
        ReleaseError::Denied
    ));
    assert!(matches!(
        released_after(FakeOutcome::key_denied()).await.unwrap_err(),
        ReleaseError::KeyDenied
    ));
    assert!(matches!(
        released_after(FakeOutcome::unavailable()).await.unwrap_err(),
        ReleaseError::Unavailable
    ));
    assert!(matches!(
        released_after(FakeOutcome::integrity_failure()).await.unwrap_err(),
        ReleaseError::IntegrityFailure
    ));
    assert!(matches!(
        released_after(FakeOutcome::internal()).await.unwrap_err(),
        ReleaseError::Internal
    ));
    assert!(matches!(
        released_after(FakeOutcome::throttled(Duration::from_secs(2))).await.unwrap_err(),
        ReleaseError::Throttled { retry_after } if retry_after == Duration::from_secs(2)
    ));
}

#[tokio::test]
async fn refuses_a_value_sealed_by_a_service_it_did_not_pin() {
    let fake = FakeSecretsService::new();
    fake.script(FakeOutcome::release(secret(), in_a_minute(), BUNDLE_GENERATION));
    let client = ReleaseClient::new(
        fake.clone(),
        clock(),
        ReleaseClientConfig::new(runtime_service(), pinned(&XKey::new())),
    );

    let refused = client.release(&request(&tenant())).await.unwrap_err();

    assert!(
        matches!(refused, ReleaseError::Unseal(UnsealError::NotOpened(source)) if source.kind() == ErrorKind::InvalidPayload)
    );
}

#[tokio::test]
async fn refuses_a_value_already_expired_when_it_arrives() {
    let refused = released_after(FakeOutcome::release(
        secret(),
        now() - Duration::from_secs(1),
        BUNDLE_GENERATION,
    ))
    .await
    .unwrap_err();

    assert!(matches!(refused, ReleaseError::Expired));
}

#[tokio::test]
async fn opens_a_value_released_by_the_pinned_service() {
    let released = released_after(FakeOutcome::release(secret(), in_a_minute(), BUNDLE_GENERATION))
        .await
        .unwrap();

    assert_eq!(released.value().as_str(), VALUE);
    assert_eq!(released.valid_until(), in_a_minute());
    assert_eq!(released.bundle_generation(), BUNDLE_GENERATION);
    assert_eq!(released.hold_until(), in_a_minute());
}

#[tokio::test]
async fn holds_a_long_lived_value_no_longer_than_the_hold_bound() {
    let released = released_after(FakeOutcome::release(
        secret(),
        now() + Duration::from_secs(3_600),
        BUNDLE_GENERATION,
    ))
    .await
    .unwrap();

    assert_eq!(released.hold_until(), now() + RELEASE_HOLD_BOUND);
}

#[tokio::test]
async fn asks_inside_platform_on_the_tenant_and_runtime_service_subject() {
    let fake = FakeSecretsService::new();
    let tenant = tenant();
    fake.script(FakeOutcome::release(secret(), in_a_minute(), BUNDLE_GENERATION));

    against_fake(&fake).release(&request(&tenant)).await.unwrap();

    let received = fake.received();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].subject(), subject(&tenant));
}

#[tokio::test]
async fn asks_for_the_credential_destination_and_location_it_was_given() {
    let fake = FakeSecretsService::new();
    fake.script(FakeOutcome::release(secret(), in_a_minute(), BUNDLE_GENERATION));

    against_fake(&fake).release(&request(&tenant())).await.unwrap();

    let asked = fake.received()[0].request().clone();
    assert_eq!(asked.credential.owner_id, OWNER);
    assert_eq!(asked.credential.credential_id, CREDENTIAL_ID);
    assert_eq!(asked.destination_host.host, HOST);
    assert_eq!(asked.destination_host.port, u32::from(PORT));
    assert_eq!(asked.location.value, Some(Value::Header(HEADER.to_owned())));
    assert_eq!(asked.correlation_id.value, CORRELATION_ID);
}

#[tokio::test]
async fn seals_each_call_to_an_xkey_of_its_own() {
    let fake = FakeSecretsService::new();
    let client = against_fake(&fake);
    let tenant = tenant();

    let _ = client.release(&request(&tenant)).await;
    let _ = client.release(&request(&tenant)).await;

    let received = fake.received();
    let first = &received[0].request().requester_xkey.public_key;
    let second = &received[1].request().requester_xkey.public_key;
    assert!(XKey::from_public_key(first).is_ok());
    assert!(XKey::from_public_key(second).is_ok());
    assert_ne!(first, second);
}

#[tokio::test]
async fn sends_the_request_as_protobuf() {
    let mock = AdvancedMockNatsClient::new();
    let tenant = tenant();
    respond_on_mock(&mock, &tenant, &ResolveForReleaseResponse::default());

    let _ = against_mock(&mock).release(&request(&tenant)).await;

    let headers: Vec<HeaderMap> = mock.requested_headers();
    assert_eq!(
        headers[0].get(HEADER_CONTENT_TYPE).map(|value| value.as_str()),
        Some(CONTENT_TYPE_PROTOBUF)
    );
}
