use std::time::{Duration, SystemTime};

use buffa::Message as _;
use buffa_types::google::protobuf::Timestamp;
use grpc_nats_micro::ContentType;
use trogonai_proto::secrets::exchange::v1alpha1::resolve_for_release_response::Result as Outcome;
use trogonai_proto::secrets::exchange::v1alpha1::{ResolveFailureCategory, ResolveForReleaseResponse};

use super::*;
use crate::SealedValue;

fn decoded(reply: &ReleaseReply) -> Outcome {
    let bytes = reply.encode(ContentType::Protobuf).unwrap();
    ResolveForReleaseResponse::decode_from_slice(&bytes)
        .unwrap()
        .result
        .unwrap()
}

#[test]
fn each_refusal_answers_its_contract_category_and_no_delay() {
    for (error, category) in [
        (
            CredentialReleaseError::Denied,
            ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_DENIED,
        ),
        (
            CredentialReleaseError::KeyDenied,
            ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_KEY_DENIED,
        ),
        (
            CredentialReleaseError::Unavailable,
            ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_UNAVAILABLE,
        ),
        (
            CredentialReleaseError::IntegrityFailure,
            ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_INTEGRITY_FAILURE,
        ),
        (
            CredentialReleaseError::Internal,
            ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_INTERNAL,
        ),
    ] {
        let Outcome::Failure(failure) = decoded(&ReleaseReply::of(Err(error))) else {
            panic!("{error:?} answered an access");
        };
        assert_eq!(failure.category.as_known(), Some(category), "{error:?}");
        assert!(failure.retry_after.is_unset(), "{error:?}");
    }
}

#[test]
fn a_release_answers_its_sealed_value_generation_and_expiry() {
    let valid_until = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_300);
    let release = SealedRelease::new(SealedValue::new(vec![1, 2, 3]).unwrap(), valid_until);
    let Outcome::Access(access) = decoded(&ReleaseReply::of(Ok(release))) else {
        panic!("a release answered a failure");
    };
    assert_eq!(access.xkey_sealed_value, vec![1, 2, 3]);
    assert_eq!(access.bundle_generation, crate::UNBUNDLED_GENERATION);
    assert_eq!(access.valid_until.as_option(), Some(&Timestamp::from(valid_until)));
}

#[test]
fn a_reply_encodes_in_either_contract_encoding() {
    let reply = ReleaseReply::of(Err(CredentialReleaseError::Denied));
    let json = reply.encode(ContentType::Json).unwrap();
    let decoded: ResolveForReleaseResponse = ContentType::Json.decode(&json).unwrap();
    assert_eq!(
        decoded,
        ResolveForReleaseResponse::decode_from_slice(&reply.encode(ContentType::Protobuf).unwrap()).unwrap()
    );
}
