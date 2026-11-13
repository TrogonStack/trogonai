use buffa::Message;
use serde_json::json;

use super::assert_json_codec;
use crate::secrets::exchange::v1alpha1::release_location::Value as LocationValue;
use crate::secrets::exchange::v1alpha1::resolve_for_release_response::Result as ReleaseResult;
use crate::secrets::exchange::v1alpha1::{
    ResolveFailure, ResolveFailureCategory, ResolveForReleaseRequest, ResolveForReleaseResponse, SealedAccess,
};

const CREDENTIAL_ID: &str = "3f2b8c1e-5d4a-4e6f-9a7b-1c2d3e4f5a6b";
const REQUESTER_XKEY: &str = "XAABBCCDDEEFFGGHHIIJJKKLLMMNNOOPPQQRRSSTTUUVVWWXXYYZZ234";

#[test]
fn resolve_for_release_request_round_trips_through_json_and_wire() {
    let request = assert_json_codec::<ResolveForReleaseRequest>(json!({
        "credential": {"credentialId": CREDENTIAL_ID, "ownerId": "project_1"},
        "destinationHost": {"host": "api.example.com", "port": 443},
        "location": {"header": "Authorization"},
        "correlationId": {"value": "corr-1"},
        "requesterXkey": {"publicKey": REQUESTER_XKEY}
    }));

    assert_eq!(request.credential.credential_id, CREDENTIAL_ID);
    assert_eq!(request.credential.owner_id, "project_1");
    assert_eq!(request.destination_host.host, "api.example.com");
    assert_eq!(request.destination_host.port, 443);
    assert_eq!(
        request.location.value,
        Some(LocationValue::Header("Authorization".to_string()))
    );
    assert_eq!(request.correlation_id.value, "corr-1");
    assert_eq!(request.requester_xkey.public_key, REQUESTER_XKEY);
}

#[test]
fn resolve_for_release_request_keeps_each_release_location_arm() {
    for (location, expected) in [
        (
            json!({"queryParameter": "api_key"}),
            LocationValue::QueryParameter("api_key".to_string()),
        ),
        (
            json!({"bodyField": "auth.token"}),
            LocationValue::BodyField("auth.token".to_string()),
        ),
    ] {
        let request = assert_json_codec::<ResolveForReleaseRequest>(json!({
            "credential": {"credentialId": CREDENTIAL_ID, "ownerId": "static-config"},
            "destinationHost": {"host": "api.example.com", "port": 8443},
            "location": location,
            "correlationId": {"value": "corr-2"},
            "requesterXkey": {"publicKey": REQUESTER_XKEY}
        }));

        assert_eq!(request.location.value, Some(expected));
    }
}

#[test]
fn sealed_access_response_round_trips_through_json_and_wire() {
    let response = assert_json_codec::<ResolveForReleaseResponse>(json!({
        "access": {
            "xkeySealedValue": "c2VhbGVk",
            "bundleGeneration": "3",
            "validUntil": "2026-11-13T12:05:00Z"
        }
    }));

    let Some(ReleaseResult::Access(access)) = response.result else {
        panic!("expected the access arm, got {:?}", response.result);
    };
    assert_eq!(&access.xkey_sealed_value[..], b"sealed");
    assert_eq!(access.bundle_generation, 3);
}

#[test]
fn failure_response_round_trips_through_json_and_wire() {
    let response = assert_json_codec::<ResolveForReleaseResponse>(json!({
        "failure": {"category": "RESOLVE_FAILURE_CATEGORY_DENIED"}
    }));

    let Some(ReleaseResult::Failure(failure)) = response.result else {
        panic!("expected the failure arm, got {:?}", response.result);
    };
    assert_eq!(
        failure.category,
        ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_DENIED
    );
    assert!(failure.retry_after.is_unset());
}

#[test]
fn sealed_access_round_trips_through_json_and_wire() {
    let access = assert_json_codec::<SealedAccess>(json!({
        "xkeySealedValue": "c2VhbGVk",
        "bundleGeneration": "1",
        "validUntil": "2026-11-13T12:05:00Z"
    }));

    assert_eq!(access.valid_until.seconds, 1_794_571_500);
}

#[test]
fn unbundled_sealed_access_keeps_generation_zero_on_json_and_wire() {
    let access = assert_json_codec::<SealedAccess>(json!({
        "xkeySealedValue": "c2VhbGVk",
        "bundleGeneration": "0",
        "validUntil": "2026-11-13T12:05:00Z"
    }));

    assert_eq!(access.bundle_generation, 0);
    let wire = access.encode_to_vec();
    assert!(
        wire.starts_with(&[0x0a, 0x06, b's', b'e', b'a', b'l', b'e', b'd', 0x10, 0x00]),
        "a required generation of zero must still be written: {wire:02x?}"
    );
}

#[test]
fn throttled_failure_round_trips_through_json_and_wire() {
    let failure = assert_json_codec::<ResolveFailure>(json!({
        "category": "RESOLVE_FAILURE_CATEGORY_THROTTLED",
        "retryAfter": "30s"
    }));

    assert_eq!(
        failure.category,
        ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_THROTTLED
    );
    assert_eq!(failure.retry_after.seconds, 30);
}

#[test]
fn truncated_resolve_for_release_request_fails_closed() {
    let wire = [0x0a, 0x0c, 0x0a, 0x04, b'0', b'1'];

    assert!(ResolveForReleaseRequest::decode_from_slice(&wire).is_err());
}
