use std::time::Duration;

use async_nats::jetstream::context::CreateStreamErrorKind;
use async_nats::jetstream::stream::{Config, DiscardPolicy, RetentionPolicy, StorageType};
use trogon_nats::test_support::JetStreamTestServer;

use super::*;
use crate::audit_stream_mismatch_error::AuditStreamMismatchError;

fn one() -> NonZeroUsize {
    NonZeroUsize::MIN
}

/// What `STREAM.CREATE` answers when the name is taken by another configuration.
fn name_in_use() -> CreateStreamError {
    let source: async_nats::jetstream::Error = serde_json::from_str(
        r#"{"code":400,"err_code":10058,"description":"stream name already in use with a different configuration"}"#,
    )
    .unwrap();
    CreateStreamError::new(CreateStreamErrorKind::JetStream(source))
}

#[tokio::test]
async fn creates_both_streams_with_the_required_configuration() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let streams = ensure_audit_streams(&js, one()).await.unwrap();

    assert_eq!(streams.len(), 2);
    for (created, kind) in streams.iter().zip(AuditStream::ALL) {
        let config = js.get_stream(kind.name()).await.unwrap().cached_info().config.clone();
        assert_eq!(created.cached_info().config.name, kind.name());
        assert_eq!(config.subjects, kind.subjects());
        assert_eq!(config.storage, StorageType::File);
        assert_eq!(config.retention, RetentionPolicy::Limits);
        assert_eq!(config.discard, DiscardPolicy::New);
        assert!(config.deny_delete);
        assert!(config.deny_purge);
        assert_eq!(config.duplicate_window, Duration::from_secs(120));
        assert!(config.max_age.is_zero());
        assert_eq!(config.num_replicas, 1);
    }
}

#[tokio::test]
async fn provisioning_twice_changes_nothing() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    ensure_audit_streams(&js, one()).await.unwrap();
    let before = js.get_stream("AUDIT").await.unwrap().cached_info().created;

    let again = ensure_audit_stream(&js, AuditStream::Audit, one()).await.unwrap();

    assert_eq!(again.cached_info().created, before);
    assert_eq!(again.cached_info().state.messages, 0);
}

#[tokio::test]
async fn concurrent_provisioners_both_succeed() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let (first, second) = tokio::join!(
        ensure_audit_stream(&js, AuditStream::Audit, one()),
        ensure_audit_stream(&js, AuditStream::Audit, one()),
    );

    assert_eq!(first.unwrap().cached_info().config.name, "AUDIT");
    assert_eq!(second.unwrap().cached_info().config.name, "AUDIT");
}

#[tokio::test]
async fn refuses_a_weakened_stream_and_leaves_it_as_it_found_it() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    type Weaken = fn(&mut Config);
    let cases: [(&str, Weaken, AuditStreamMismatchError); 6] = [
        (
            "discard_old",
            |c| c.discard = DiscardPolicy::Old,
            AuditStreamMismatchError::Discard { stream: "AUDIT" },
        ),
        (
            "allow_delete",
            |c| c.deny_delete = false,
            AuditStreamMismatchError::DenyDelete { stream: "AUDIT" },
        ),
        (
            "allow_purge",
            |c| c.deny_purge = false,
            AuditStreamMismatchError::DenyPurge { stream: "AUDIT" },
        ),
        (
            "max_age_one_hour",
            |c| c.max_age = Duration::from_secs(3600),
            AuditStreamMismatchError::MaxAge { stream: "AUDIT" },
        ),
        (
            "memory_storage",
            |c| c.storage = StorageType::Memory,
            AuditStreamMismatchError::Storage { stream: "AUDIT" },
        ),
        (
            "missing_credential_subject",
            |c| c.subjects.truncate(1),
            AuditStreamMismatchError::MissingSubject {
                stream: "AUDIT",
                subject: "platform.v1.audit.credential.>".to_owned(),
            },
        ),
    ];
    for (case, weaken, expected) in cases {
        let mut weakened = AuditStream::Audit.config(one());
        weaken(&mut weakened);
        if let Err(error) = js.create_stream(weakened.clone()).await {
            let diagnostics = server.failure_diagnostics().await;
            let status = match error.kind() {
                CreateStreamErrorKind::JetStream(error) => {
                    format!("status={} jetstream_code={}", error.code(), error.error_code().0)
                }
                CreateStreamErrorKind::TimedOut => "request_timeout".to_owned(),
                _ => "other_create_error".to_owned(),
            };
            panic!(
                "weakened audit stream fixture create failed: case={case} {status}; ensure_audit_stream_not_attempted\n{diagnostics}"
            );
        }

        let error = ensure_audit_stream(&js, AuditStream::Audit, one()).await.unwrap_err();

        assert!(
            matches!(&error, EnsureAuditStreamError::ConfigMismatch(found) if *found == expected),
            "expected {expected:?}, found {error:?}"
        );
        let kept = js.get_stream("AUDIT").await.unwrap().cached_info().config.clone();
        assert_eq!(kept.subjects, weakened.subjects, "{expected:?}");
        assert_eq!(kept.discard, weakened.discard, "{expected:?}");
        assert_eq!(kept.deny_delete, weakened.deny_delete, "{expected:?}");
        assert_eq!(kept.deny_purge, weakened.deny_purge, "{expected:?}");
        assert_eq!(kept.max_age, weakened.max_age, "{expected:?}");
        js.delete_stream("AUDIT").await.unwrap();
    }
}

#[tokio::test]
async fn refuses_a_stream_with_fewer_replicas_than_asked_for() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    ensure_audit_stream(&js, AuditStream::Checkpoint, one()).await.unwrap();

    let error = ensure_audit_stream(&js, AuditStream::Checkpoint, NonZeroUsize::new(3).unwrap())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        EnsureAuditStreamError::ConfigMismatch(AuditStreamMismatchError::Replicas {
            stream: "AUDIT_CHECKPOINT"
        })
    ));
}

#[tokio::test]
async fn a_create_the_server_refuses_is_a_create_error() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let error = ensure_audit_stream(&js, AuditStream::Audit, NonZeroUsize::new(3).unwrap())
        .await
        .unwrap_err();

    assert!(matches!(error, EnsureAuditStreamError::Create { stream: "AUDIT", .. }));
    assert!(js.get_stream("AUDIT").await.is_err());
}

#[tokio::test]
async fn a_lookup_that_fails_for_another_reason_than_absence_is_a_get_error() {
    let server = JetStreamTestServer::start().await;
    let js = async_nats::jetstream::with_prefix(server.client().await, "no.such.api");

    let error = ensure_audit_stream(&js, AuditStream::Audit, one()).await.unwrap_err();

    assert!(matches!(error, EnsureAuditStreamError::Get { stream: "AUDIT", .. }));
}

#[tokio::test]
async fn a_create_that_lost_the_race_validates_the_stream_that_won() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    ensure_audit_stream(&js, AuditStream::Audit, one()).await.unwrap();

    let settled = settle_create(&js, AuditStream::Audit, one(), Err(name_in_use())).await;

    assert_eq!(settled.unwrap().cached_info().config.name, "AUDIT");
}

#[tokio::test]
async fn a_create_that_lost_the_race_to_a_weakened_stream_is_refused() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let mut weakened = AuditStream::Audit.config(one());
    weakened.discard = DiscardPolicy::Old;
    js.create_stream(weakened).await.unwrap();

    let error = settle_create(&js, AuditStream::Audit, one(), Err(name_in_use()))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        EnsureAuditStreamError::ConfigMismatch(AuditStreamMismatchError::Discard { stream: "AUDIT" })
    ));
}

#[tokio::test]
async fn a_conflict_over_a_stream_that_cannot_be_read_back_is_a_get_error() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let error = settle_create(&js, AuditStream::Audit, one(), Err(name_in_use()))
        .await
        .unwrap_err();

    assert!(matches!(error, EnsureAuditStreamError::Get { stream: "AUDIT", .. }));
}
