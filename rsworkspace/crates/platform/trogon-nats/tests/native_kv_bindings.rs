#![allow(clippy::unwrap_used)]
use async_nats::jetstream::context::{PublishError, PublishErrorKind};
use bytes::Bytes;
use std::num::NonZeroUsize;
use trogon_nats::{
    NatsToken,
    jetstream::{NativeKvCasError, NativeKvCasStage, NativeKvExpectation, NativeKvKey, NativeKvScope, NativeKvUpdate},
};

#[test]
fn native_kv_scope_refuses_buckets_whose_stream_name_cannot_be_used() {
    let scope = NativeKvScope::new(NatsToken::new("b".repeat(125)).unwrap()).unwrap();
    assert!(NatsToken::new(scope.stream()).is_ok());
    for length in 126..=128 {
        assert!(matches!(
            NativeKvScope::new(NatsToken::new("b".repeat(length)).unwrap()),
            Err(NativeKvCasError::InvalidBinding)
        ));
    }
}

#[test]
fn native_kv_values_preserve_the_exact_request_and_sequence_bounds() {
    let scope = NativeKvScope::new(NatsToken::new("bucket_1").unwrap()).unwrap();
    let key = NativeKvKey::new("path/to.value=1").unwrap();
    let value = Bytes::from_static(b"original bytes");
    let expectation = NativeKvExpectation::from_sequences(7, 9).unwrap();
    let maximum = NonZeroUsize::new(4096).unwrap();
    let request = NativeKvUpdate::new(&scope, &key, value.clone(), expectation, maximum);
    assert_eq!(request.scope(), &scope);
    assert_eq!(request.key(), &key);
    assert_eq!(request.value(), &value);
    assert_eq!(request.expectation(), expectation);
    assert_eq!(request.maximum_envelope_bytes(), maximum);
    assert_eq!(scope.bucket().as_str(), "bucket_1");
    assert_eq!(scope.stream(), "KV_bucket_1");
    assert_eq!(scope.subject(&key), "$KV.bucket_1.path/to.value=1");
    assert_eq!(expectation.key_sequence(), 7);
    assert_eq!(expectation.global_sequence(), 9);
    let largest = NativeKvExpectation::from_sequences(u64::MAX - 1, u64::MAX - 1).unwrap();
    assert_eq!(largest.key_sequence(), u64::MAX - 1);
    assert_eq!(largest.global_sequence(), u64::MAX - 1);
    assert!(matches!(
        NativeKvExpectation::from_sequences(0, u64::MAX),
        Err(NativeKvCasError::InvalidExpectation)
    ));
    assert!(matches!(
        NativeKvScope::new(NatsToken::new("invalid/bucket").unwrap()),
        Err(NativeKvCasError::InvalidBinding)
    ));
    assert!(NativeKvKey::new("k".repeat(1024)).is_ok());
    assert!(matches!(
        NativeKvKey::new("k".repeat(1025)),
        Err(NativeKvCasError::InvalidKey)
    ));
}

#[test]
fn uncertain_native_publications_preserve_typed_sources_without_debugging_them() {
    for (stage, rendered_stage) in [
        (NativeKvCasStage::Publish, "Publish"),
        (NativeKvCasStage::Acknowledge, "Acknowledge"),
    ] {
        let error = NativeKvCasError::Uncertain {
            stage,
            source: PublishError::new(PublishErrorKind::Other),
        };
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(
            format!("{error:?}"),
            format!("Uncertain {{ stage: {rendered_stage}, .. }}")
        );
        assert_eq!(
            error.to_string(),
            format!("conditional KV publication is uncertain at {rendered_stage}")
        );
    }
    assert_eq!(
        format!("{:?}", NativeKvCasError::Conflict),
        "conditional KV global or key sequence changed"
    );
}
