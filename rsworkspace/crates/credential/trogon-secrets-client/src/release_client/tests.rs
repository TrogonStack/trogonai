use async_nats::HeaderMap;
use buffa::Message;
use bytes::Bytes;
use grpc_nats_micro::constants::{CONTENT_TYPE_PROTOBUF, HEADER_CONTENT_TYPE};
use grpc_nats_micro::status_codec::encode_reply;
use grpc_nats_micro::{ContentType, Outcome, ReplyError, ServiceErrorCode, ServiceFault};
use nkeys::XKey;
use trogon_nats::mocks::AdvancedMockNatsClient;
use trogon_std::time::FixedEpochClock;
use trogonai_proto::secrets::exchange::v1alpha1::{
    ResolveFailure, ResolveFailureCategory as Category, ResolveForReleaseResponse,
};

use super::ReleaseClient;
use crate::fixtures::{clock, pinned, request, runtime_service, tenant};
use crate::{InvalidReleaseResponse, ReleaseClientConfig, ReleaseError, TenantAccount};

fn subject(tenant: &TenantAccount) -> String {
    format!("{tenant}.secrets.v1.service.billing-runner.SecretExchangeService.ResolveForRelease")
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

    assert!(matches!(
        refused,
        ReleaseError::InvalidResponse(InvalidReleaseResponse::Reply(ReplyError::Decode(_)))
    ));
}

#[tokio::test]
async fn refuses_a_reply_with_no_result() {
    let mock = AdvancedMockNatsClient::new();
    let tenant = tenant();
    respond_on_mock(&mock, &tenant, &ResolveForReleaseResponse::default());

    let refused = against_mock(&mock).release(&request(&tenant)).await.unwrap_err();

    assert!(matches!(
        refused,
        ReleaseError::InvalidResponse(InvalidReleaseResponse::MissingResult)
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
        ReleaseError::InvalidResponse(InvalidReleaseResponse::ThrottledWithoutRetryAfter)
    ));
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
