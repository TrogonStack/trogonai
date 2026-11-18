#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use async_nats::jetstream::kv::Operation;
use buffa::Message as _;
use bytes::Bytes;
use trogon_nats::jetstream::MockJetStreamKvStore;
use trogon_webhooks::{
    AuthenticationMode, EndpointId, EndpointLookup, EndpointRegistration, EndpointStatus, NatsEndpointRegistry,
    ProjectId, TimestampTolerance, WebhookBodySize,
};
use trogonai_proto::gateway::webhooks::v1;

const ENDPOINT: &str = "67b82e3f-5c99-486b-b8a0-acdad9548e3d";

fn record() -> v1::EndpointRegistration {
    v1::EndpointRegistration {
        endpoint_id: ENDPOINT.to_owned(),
        project_id: "project-1".to_owned(),
        status: v1::EndpointStatus::Enabled.into(),
        authentication_mode: v1::AuthenticationMode::Bearer.into(),
        max_body_bytes: 4096,
        timestamp_tolerance_seconds: 300,
    }
}

#[tokio::test]
async fn registry_reads_current_protobuf_record_and_uses_storage_revision() {
    let store = MockJetStreamKvStore::new();
    store.enqueue_entry(Bytes::from(record().encode_to_vec()), 42, Operation::Put);
    let endpoint = NatsEndpointRegistry::new(store)
        .lookup(&EndpointId::new(ENDPOINT).unwrap())
        .await
        .unwrap()
        .expect("registered endpoint");
    assert_eq!(endpoint.registration().endpoint_id().as_str(), ENDPOINT);
    assert_eq!(endpoint.registration().project_id().as_str(), "project-1");
    assert_eq!(endpoint.registration().status(), EndpointStatus::Enabled);
    assert_eq!(
        endpoint.registration().authentication_mode(),
        AuthenticationMode::Bearer
    );
    assert_eq!(endpoint.registration().max_body_size().get(), 4096);
    assert_eq!(endpoint.configuration_revision().get(), 42);
}

#[tokio::test]
async fn registry_preserves_transient_backend_failure_and_recovers_on_the_next_read() {
    let registry = NatsEndpointRegistry::new(MockJetStreamKvStore::new());
    registry
        .store()
        .enqueue_entry_error(async_nats::jetstream::kv::EntryErrorKind::TimedOut);
    let error = registry.lookup(&EndpointId::new(ENDPOINT).unwrap()).await.unwrap_err();
    assert!(matches!(
        error,
        trogon_webhooks::RegistryError::Read(source)
            if source.kind() == async_nats::jetstream::kv::EntryErrorKind::TimedOut
    ));

    registry
        .store()
        .enqueue_entry(Bytes::from(record().encode_to_vec()), 43, Operation::Put);
    let recovered = registry
        .lookup(&EndpointId::new(ENDPOINT).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.configuration_revision().get(), 43);
    assert_eq!(recovered.registration().project_id().as_str(), "project-1");
}

#[tokio::test]
async fn registry_does_not_reuse_deleted_or_purged_records() {
    for operation in [Operation::Delete, Operation::Purge] {
        let store = MockJetStreamKvStore::new();
        store.enqueue_entry(Bytes::from(record().encode_to_vec()), 42, operation);
        assert!(
            NatsEndpointRegistry::new(store)
                .lookup(&EndpointId::new(ENDPOINT).unwrap())
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn registry_rejects_unknown_enum_values_invalid_domains_and_identity_mismatch() {
    let mut missing_mode = record();
    missing_mode.authentication_mode = v1::AuthenticationMode::Unspecified.into();
    let mut missing_status = record();
    missing_status.status = v1::EndpointStatus::Unspecified.into();
    let mut invalid_project = record();
    invalid_project.project_id = "project.*".to_owned();
    let mut mismatch = record();
    mismatch.endpoint_id = "2296254b-3f32-40fa-9158-733e5cb6fb94".to_owned();
    let mut oversized_limit = record();
    oversized_limit.max_body_bytes = 1_048_577;
    let mut zero_limit = record();
    zero_limit.max_body_bytes = 0;
    let mut invalid_tolerance = record();
    invalid_tolerance.timestamp_tolerance_seconds = 0;
    let mut unknown_mode = record();
    unknown_mode.authentication_mode = buffa::EnumValue::from(999);
    let mut unknown_status = record();
    unknown_status.status = buffa::EnumValue::from(999);
    for record in [
        missing_mode,
        missing_status,
        invalid_project,
        mismatch,
        oversized_limit,
        zero_limit,
        invalid_tolerance,
        unknown_mode,
        unknown_status,
    ] {
        let store = MockJetStreamKvStore::new();
        store.enqueue_entry(Bytes::from(record.encode_to_vec()), 1, Operation::Put);
        assert!(
            NatsEndpointRegistry::new(store)
                .lookup(&EndpointId::new(ENDPOINT).unwrap())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn registry_rejects_oversized_malformed_and_zero_revision_records() {
    for (bytes, revision) in [(vec![0xff; 4097], 1), (vec![0xff], 1), (record().encode_to_vec(), 0)] {
        let store = MockJetStreamKvStore::new();
        store.enqueue_entry(Bytes::from(bytes), revision, Operation::Put);
        assert!(
            NatsEndpointRegistry::new(store)
                .lookup(&EndpointId::new(ENDPOINT).unwrap())
                .await
                .is_err()
        );
    }
}

#[test]
fn domain_values_bound_endpoint_project_payload_and_clock_inputs() {
    assert!(EndpointId::new("67B82E3F-5C99-486B-B8A0-ACDAD9548E3D").is_err());
    assert!(EndpointId::new("00000000-0000-0000-0000-000000000000").is_err());
    assert!(trogon_webhooks::ProjectId::new("project.>").is_err());
    assert!(trogon_webhooks::ProjectId::new("project\0").is_err());
    assert!(trogon_webhooks::ProjectId::new(&"a".repeat(65)).is_err());
    assert!(trogon_webhooks::WebhookBodySize::new(0).is_err());
    assert!(trogon_webhooks::WebhookBodySize::new(1_048_577).is_err());
    assert!(trogon_webhooks::TimestampTolerance::new(0).is_err());
    assert!(trogon_webhooks::TimestampTolerance::new(3601).is_err());
    assert!(trogon_webhooks::ConfigurationRevision::new(0).is_err());
}

#[test]
fn registration_encoding_round_trips_validated_authentication_and_status() {
    for mode in [AuthenticationMode::Bearer, AuthenticationMode::HmacSha256] {
        for status in [EndpointStatus::Enabled, EndpointStatus::Disabled] {
            let registration = EndpointRegistration::new(
                EndpointId::new(ENDPOINT).unwrap(),
                ProjectId::new("project-1").unwrap(),
                status,
                mode,
                WebhookBodySize::new(4096).unwrap(),
                TimestampTolerance::new(300).unwrap(),
            );
            let wire = v1::EndpointRegistration::decode_from_slice(&registration.encode()).unwrap();
            assert_eq!(EndpointRegistration::try_from(wire).unwrap(), registration);
        }
    }
}
