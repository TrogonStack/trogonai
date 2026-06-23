    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::a2a_prefix::A2aPrefix;
    use crate::constants::DEFAULT_PUSH_DLQ_CALLER_SEGMENT;
    use crate::push::caller_id::resolve_push_dlq_caller_id;
    use crate::push::dispatch_error::DispatchError;
    use crate::push::push_notification_target::PushNotificationTargetError;
    use crate::push::terminal_push_task_state::TerminalPushTaskState;
    use bytes::Bytes;
    use trogon_nats::jetstream::JetStreamPublisher;

    #[derive(Clone, Default)]
    struct RecordingPublisher {
        publishes: Arc<Mutex<Vec<(String, HeaderMap, Bytes)>>>,
    }

    impl JetStreamPublisher for RecordingPublisher {
        type PublishError = std::io::Error;
        type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

        async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
            &self,
            subject: S,
            headers: HeaderMap,
            payload: Bytes,
        ) -> Result<Self::AckFuture, Self::PublishError> {
            self.publishes
                .lock()
                .unwrap()
                .push((subject.to_subject().to_string(), headers, payload));
            Ok(std::future::ready(Ok(async_nats::jetstream::publish::PublishAck {
                duplicate: false,
                stream: "A2A_PUSH_DLQ".into(),
                sequence: 1,
                domain: String::new(),
                value: None,
            })))
        }
    }

    /// Publisher whose `publish_with_headers` returns an immediate transport error.
    #[derive(Clone, Default)]
    struct FailingPublisher;

    impl JetStreamPublisher for FailingPublisher {
        type PublishError = std::io::Error;
        type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

        async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
            &self,
            _subject: S,
            _headers: HeaderMap,
            _payload: Bytes,
        ) -> Result<Self::AckFuture, Self::PublishError> {
            Err(std::io::Error::other("transport down"))
        }
    }

    /// Publisher whose publish succeeds but whose ack future resolves to an error.
    #[derive(Clone, Default)]
    struct AckFailingPublisher;

    impl JetStreamPublisher for AckFailingPublisher {
        type PublishError = std::io::Error;
        type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

        async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
            &self,
            _subject: S,
            _headers: HeaderMap,
            _payload: Bytes,
        ) -> Result<Self::AckFuture, Self::PublishError> {
            Ok(std::future::ready(Err(std::io::Error::other("ack timeout"))))
        }
    }

    fn prefix() -> A2aPrefix {
        A2aPrefix::new("a2a".to_string()).unwrap()
    }

    fn sample_config() -> a2a::types::TaskPushNotificationConfig {
        a2a::types::TaskPushNotificationConfig {
            id: Some("pcfg-1".to_string()),
            url: "https://example.com/webhook".to_string(),
            task_id: String::new(),
            token: None,
            authentication: None,
            tenant: None,
        }
    }

    #[test]
    fn push_dlq_subject_default_caller_round_trips_expected_pattern() {
        let prefix = A2aPrefix::new("a2a".to_string()).unwrap();
        let tid = A2aTaskId::new("task7").unwrap();
        assert_eq!(
            push_dlq_publish_subject(&prefix, &CallerId::default(), &tid),
            "a2a.push.dlq._.task7"
        );
    }

    #[test]
    fn push_dlq_subject_includes_derived_principal_caller_segment() {
        let prefix = A2aPrefix::new("a2a".to_string()).unwrap();
        let tid = A2aTaskId::new("task7").unwrap();
        let cid = CallerId::from_principal(&a2a_identity_types::SpiceDbPrincipal(
            serde_json::json!({"spicedb_subject": "c1.d2"}),
        ));
        assert_eq!(
            push_dlq_publish_subject(&prefix, &cid, &tid),
            "a2a.push.dlq.c1_d2.task7"
        );
    }

    #[test]
    fn sanitize_replaces_spaces_and_dots() {
        assert_eq!(sanitize_subject_token(" u1.id ").as_ref(), "u1_id",);
        assert_eq!(sanitize_subject_token("").as_ref(), "_");
    }

    #[test]
    fn notification_body_json_preserves_objects() {
        let v = notification_body_json(br#"{"a":1}"#);
        assert_eq!(v, serde_json::json!({"a": 1}));
    }

    #[test]
    fn notification_body_json_fallback_to_string_for_non_utfish() {
        let value = notification_body_json(&[0xffu8, 0xfe]);
        assert!(matches!(value, serde_json::Value::String(ref s) if s.contains('\u{fffd}')));
    }

    #[test]
    fn resolve_absent_principal_keeps_fallback_caller_segment() {
        let fallback = CallerId::default();
        assert_eq!(
            resolve_push_dlq_caller_id(None, &fallback).as_str(),
            DEFAULT_PUSH_DLQ_CALLER_SEGMENT
        );
    }

    #[test]
    fn resolve_principal_with_spicedb_subject_builds_dlq_subject() {
        let prefix = A2aPrefix::new("a2a".to_string()).unwrap();
        let tid = A2aTaskId::new("task7").unwrap();
        let p = a2a_identity_types::SpiceDbPrincipal(serde_json::json!({"spicedb_subject": "c1.d2"}));
        let cid = resolve_push_dlq_caller_id(Some(&p), &CallerId::default());
        assert_eq!(
            push_dlq_publish_subject(&prefix, &cid, &tid),
            "a2a.push.dlq.c1_d2.task7"
        );
    }

    #[test]
    fn resolve_principal_without_spicedb_subject_falls_back() {
        let prefix = A2aPrefix::new("a2a".to_string()).unwrap();
        let tid = A2aTaskId::new("task7").unwrap();
        let p = a2a_identity_types::SpiceDbPrincipal(serde_json::json!({}));
        let cid = resolve_push_dlq_caller_id(Some(&p), &CallerId::default());
        assert_eq!(push_dlq_publish_subject(&prefix, &cid, &tid), "a2a.push.dlq._.task7");
    }

    #[tokio::test]
    async fn second_publish_with_same_key_is_suppressed_by_lru() {
        let js = RecordingPublisher::default();
        let dedup = PushDlqDedupGate::with_capacity(32);
        let prefix = prefix();
        let task_id = A2aTaskId::new("task-1").unwrap();
        let config = sample_config();
        let transition = StatusTransitionId::from_terminal(TerminalPushTaskState::Failed);
        let err = DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme { raw: "bad".into() });

        publish_push_delivery_failure(
            &js,
            &prefix,
            &CallerId::default(),
            &task_id,
            &config,
            br#"{}"#,
            &err,
            transition.clone(),
            &dedup,
        )
        .await;
        publish_push_delivery_failure(
            &js,
            &prefix,
            &CallerId::default(),
            &task_id,
            &config,
            br#"{}"#,
            &err,
            transition,
            &dedup,
        )
        .await;

        assert_eq!(js.publishes.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn different_transition_ids_both_publish() {
        let js = RecordingPublisher::default();
        let dedup = PushDlqDedupGate::with_capacity(32);
        let prefix = prefix();
        let task_id = A2aTaskId::new("task-1").unwrap();
        let config = sample_config();
        let err = DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme { raw: "bad".into() });

        publish_push_delivery_failure(
            &js,
            &prefix,
            &CallerId::default(),
            &task_id,
            &config,
            br#"{}"#,
            &err,
            StatusTransitionId::from_terminal(TerminalPushTaskState::Failed),
            &dedup,
        )
        .await;
        publish_push_delivery_failure(
            &js,
            &prefix,
            &CallerId::default(),
            &task_id,
            &config,
            br#"{}"#,
            &err,
            StatusTransitionId::from_terminal(TerminalPushTaskState::Completed),
            &dedup,
        )
        .await;

        assert_eq!(js.publishes.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn publish_sets_nats_msg_id_header() {
        let js = RecordingPublisher::default();
        let dedup = PushDlqDedupGate::with_capacity(32);
        let prefix = prefix();
        let task_id = A2aTaskId::new("task-1").unwrap();
        let config = sample_config();
        let err = DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme { raw: "bad".into() });

        publish_push_delivery_failure(
            &js,
            &prefix,
            &CallerId::default(),
            &task_id,
            &config,
            br#"{}"#,
            &err,
            StatusTransitionId::from_terminal(TerminalPushTaskState::Failed),
            &dedup,
        )
        .await;

        let published = js.publishes.lock().unwrap();
        assert_eq!(published.len(), 1);
        assert_eq!(
            published[0].1.get(NATS_MSG_ID_HEADER).unwrap().as_str(),
            "3:dlq|6:task-1|6:failed|27:https://example.com/webhook"
        );
    }

    #[tokio::test]
    async fn publish_path_swallows_transport_error_without_panicking() {
        let js = FailingPublisher;
        let dedup = PushDlqDedupGate::default();
        let prefix = prefix();
        let task_id = A2aTaskId::new("task-x").unwrap();
        let config = sample_config();
        let err = DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme { raw: "bad".into() });

        publish_push_delivery_failure(
            &js,
            &prefix,
            &CallerId::default(),
            &task_id,
            &config,
            br#"{}"#,
            &err,
            StatusTransitionId::from_terminal(TerminalPushTaskState::Failed),
            &dedup,
        )
        .await;
    }

    #[tokio::test]
    async fn publish_path_swallows_ack_failure_without_panicking() {
        let js = AckFailingPublisher;
        let dedup = PushDlqDedupGate::default();
        let prefix = prefix();
        let task_id = A2aTaskId::new("task-y").unwrap();
        let config = sample_config();
        let err = DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme { raw: "bad".into() });

        publish_push_delivery_failure(
            &js,
            &prefix,
            &CallerId::default(),
            &task_id,
            &config,
            br#"{}"#,
            &err,
            StatusTransitionId::from_terminal(TerminalPushTaskState::Failed),
            &dedup,
        )
        .await;
    }
