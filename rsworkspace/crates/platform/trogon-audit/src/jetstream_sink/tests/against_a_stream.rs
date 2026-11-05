//! The sink against a real JetStream server, on the streams provisioning builds.

use std::num::NonZeroUsize;

use async_nats::jetstream;
use trogon_nats::test_support::JetStreamTestServer;

use super::*;
use crate::audit_stream::AuditStream;
use crate::ensure_audit_stream::ensure_audit_stream;
use crate::fixtures::{allow, deny, stamper};

async fn provisioned() -> (JetStreamTestServer, jetstream::Context) {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    ensure_audit_stream(&js, AuditStream::Audit, NonZeroUsize::MIN)
        .await
        .unwrap();
    (server, js)
}

async fn stored_count(js: &jetstream::Context) -> u64 {
    js.get_stream("AUDIT").await.unwrap().cached_info().state.messages
}

#[tokio::test]
async fn a_sealed_record_is_stored_on_its_subject_under_its_record_id() {
    let (_server, js) = provisioned().await;
    let sink = JetStreamSink::with_defaults(js.clone());
    let record = SealedRecord::seal(allow(&stamper()), &crate::fixtures::writer_id(), 1);

    sink.publish(&record).await.unwrap();

    let stream = js.get_stream("AUDIT").await.unwrap();
    let stored = stream.get_raw_message(1).await.unwrap();
    assert_eq!(stored.subject.as_str(), record.subject());
    assert_eq!(stored.payload, *record.payload());
    let id = stored.headers.get(NATS_MESSAGE_ID).unwrap().to_string();
    assert_eq!(id, record.id().to_string());
}

#[tokio::test]
async fn republishing_the_same_record_does_not_store_it_twice() {
    let (_server, js) = provisioned().await;
    let sink = JetStreamSink::with_defaults(js.clone());
    let record = SealedRecord::seal(allow(&stamper()), &crate::fixtures::writer_id(), 1);

    sink.publish(&record).await.unwrap();
    sink.publish(&record).await.unwrap();

    assert_eq!(stored_count(&js).await, 1);
}

#[tokio::test]
async fn distinct_records_are_each_stored() {
    let (_server, js) = provisioned().await;
    let sink = JetStreamSink::with_defaults(js.clone());
    let stamper = stamper();
    let writer = crate::fixtures::writer_id();

    sink.publish(&SealedRecord::seal(allow(&stamper), &writer, 1))
        .await
        .unwrap();
    sink.publish(&SealedRecord::seal(deny(&stamper), &writer, 2))
        .await
        .unwrap();

    assert_eq!(stored_count(&js).await, 2);
}

#[tokio::test]
async fn a_full_stream_is_reported_as_full_and_not_as_unreachable() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let mut config = AuditStream::Audit.config(NonZeroUsize::MIN);
    config.max_messages = 1;
    js.create_stream(config).await.unwrap();
    let sink = JetStreamSink::with_defaults(js.clone());
    let stamper = stamper();
    let writer = crate::fixtures::writer_id();
    sink.publish(&SealedRecord::seal(allow(&stamper), &writer, 1))
        .await
        .unwrap();

    let refused = sink
        .publish(&SealedRecord::seal(deny(&stamper), &writer, 2))
        .await
        .unwrap_err();

    assert_eq!(refused, SinkError::StreamFull);
    assert_eq!(refused.reason(), crate::failure_reason::FailureReason::StreamFull);
    assert_ne!(refused, SinkError::Unreachable);
    assert_eq!(stored_count(&js).await, 1, "a full stream keeps the record it has");
}

#[tokio::test]
async fn a_stream_that_was_never_provisioned_is_unreachable() {
    let server = JetStreamTestServer::start().await;
    let sink = JetStreamSink::with_defaults(server.jetstream().await);
    let record = SealedRecord::seal(allow(&stamper()), &crate::fixtures::writer_id(), 1);

    assert_eq!(sink.publish(&record).await, Err(SinkError::Unreachable));
}
