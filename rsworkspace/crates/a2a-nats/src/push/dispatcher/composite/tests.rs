    use super::*;
    use std::sync::Mutex;

    /// Test dispatcher that records which leg was invoked, so the composite's
    /// routing rules can be asserted without spinning up real transports.
    #[derive(Default)]
    struct RecordingLeg {
        tag: &'static str,
        calls: Mutex<Vec<&'static str>>,
    }

    #[async_trait::async_trait]
    impl PushDispatcher for RecordingLeg {
        async fn dispatch(
            &self,
            _task_id: &A2aTaskId,
            _config: &TaskPushNotificationConfig,
            _delivery_semantics: DeliverySemantics,
            _terminal_task_state: TerminalPushTaskState,
            _payload: &[u8],
        ) -> Result<(), DispatchError> {
            self.calls.lock().unwrap().push(self.tag);
            Ok(())
        }
    }

    fn config(url: &str) -> TaskPushNotificationConfig {
        TaskPushNotificationConfig {
            url: url.into(),
            id: Some("cfg-1".into()),
            task_id: String::new(),
            token: None,
            authentication: None,
            tenant: None,
        }
    }

    fn task() -> A2aTaskId {
        A2aTaskId::new("task-1").unwrap()
    }

    fn http_leg() -> HttpPushDispatcher {
        HttpPushDispatcher::new(reqwest::Client::new())
    }

    #[tokio::test]
    async fn http_target_routes_to_http_leg() {
        // The HTTP leg is a real HttpPushDispatcher pointed at a reserved
        // port; the dispatcher returns Err on transport refusal, but the
        // routing decision has already happened by then — we assert that
        // the nats/jetstream legs were NOT touched.
        let nats = Arc::new(RecordingLeg {
            tag: "nats",
            ..Default::default()
        });
        let js = Arc::new(RecordingLeg {
            tag: "js",
            ..Default::default()
        });
        let composite = CompositePushDispatcher::new(
            http_leg(),
            Box::new(NatsBox(nats.clone())),
            Box::new(NatsBox(js.clone())),
        );

        let _ = composite
            .dispatch(
                &task(),
                &config("http://127.0.0.1:1/hook"),
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await;

        assert!(nats.calls.lock().unwrap().is_empty());
        assert!(js.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn nats_target_routes_to_nats_leg() {
        let nats = Arc::new(RecordingLeg {
            tag: "nats",
            ..Default::default()
        });
        let js = Arc::new(RecordingLeg {
            tag: "js",
            ..Default::default()
        });
        let composite = CompositePushDispatcher::new(
            http_leg(),
            Box::new(NatsBox(nats.clone())),
            Box::new(NatsBox(js.clone())),
        );

        composite
            .dispatch(
                &task(),
                &config("subject:a2a.push.bot.caller.t1"),
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await
            .unwrap();

        assert_eq!(*nats.calls.lock().unwrap(), vec!["nats"]);
        assert!(js.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn jetstream_target_routes_to_jetstream_leg() {
        let nats = Arc::new(RecordingLeg {
            tag: "nats",
            ..Default::default()
        });
        let js = Arc::new(RecordingLeg {
            tag: "js",
            ..Default::default()
        });
        let composite = CompositePushDispatcher::new(
            http_leg(),
            Box::new(NatsBox(nats.clone())),
            Box::new(NatsBox(js.clone())),
        );

        composite
            .dispatch(
                &task(),
                &config("jetstream:a2a.push.bot.caller.t1"),
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await
            .unwrap();

        assert_eq!(*js.calls.lock().unwrap(), vec!["js"]);
        assert!(nats.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unparseable_url_surfaces_invalid_target_error() {
        let nats = Arc::new(RecordingLeg {
            tag: "nats",
            ..Default::default()
        });
        let js = Arc::new(RecordingLeg {
            tag: "js",
            ..Default::default()
        });
        let composite = CompositePushDispatcher::new(
            http_leg(),
            Box::new(NatsBox(nats.clone())),
            Box::new(NatsBox(js.clone())),
        );

        let err = composite
            .dispatch(
                &task(),
                &config("ftp://example.com/hook"),
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidTarget(_)));
        assert!(nats.calls.lock().unwrap().is_empty());
        assert!(js.calls.lock().unwrap().is_empty());
    }

    /// Thin newtype that wraps an `Arc<RecordingLeg>` so it satisfies the
    /// `Box<dyn PushDispatcher>` slot without giving up the shared handle
    /// the test uses to assert on call counts.
    struct NatsBox(Arc<RecordingLeg>);

    #[async_trait::async_trait]
    impl PushDispatcher for NatsBox {
        async fn dispatch(
            &self,
            task_id: &A2aTaskId,
            config: &TaskPushNotificationConfig,
            delivery_semantics: DeliverySemantics,
            terminal_task_state: TerminalPushTaskState,
            payload: &[u8],
        ) -> Result<(), DispatchError> {
            self.0
                .dispatch(task_id, config, delivery_semantics, terminal_task_state, payload)
                .await
        }
    }

    #[tokio::test]
    async fn factory_returns_dyn_dispatcher_routing_by_target() {
        use trogon_nats::AdvancedMockNatsClient;

        let nats = AdvancedMockNatsClient::new();
        let js = trogon_nats::jetstream::mocks::MockJetStreamPublisher::new();
        let dispatcher = composite_push_dispatcher(nats.clone(), js, reqwest::Client::new());

        dispatcher
            .dispatch(
                &task(),
                &config("subject:a2a.push.bot.caller.t1"),
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await
            .unwrap();
        assert_eq!(nats.published_messages(), vec!["a2a.push.bot.caller.t1"]);
    }
