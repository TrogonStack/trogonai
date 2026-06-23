use std::time::Duration;

    use async_nats::jetstream::{self};
    use bytes::Bytes;
    use testcontainers_modules::nats::{Nats, NatsServerCmd};
    use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};

    use super::*;
    use crate::processor::execution::worker::dispatcher::DeliveredMessage;

    struct NatsServer {
        _container: ContainerAsync<Nats>,
        url: String,
    }

    impl NatsServer {
        async fn start() -> Self {
            let cmd = NatsServerCmd::default().with_jetstream();
            let container = Nats::default()
                .with_cmd(&cmd)
                .start()
                .await
                .expect("start NATS testcontainer for JetStream unit tests");
            let host = container.get_host().await.expect("get NATS testcontainer host");
            let port = container
                .get_host_port_ipv4(4222)
                .await
                .expect("get NATS testcontainer port");
            let url = format!("{host}:{port}");
            Self {
                _container: container,
                url,
            }
        }

        fn url(&self) -> &str {
            &self.url
        }
    }

    fn js_ack_reply(delivered: u64) -> String {
        format!("$JS.ACK._.hash.STREAM.consumer.{delivered}.10.1.1704000000000000000.0.tok")
    }

    async fn jetstream_message(delivered: u64) -> (NatsServer, jetstream::Message) {
        let server = NatsServer::start().await;
        let client = async_nats::ConnectOptions::new()
            .connection_timeout(Duration::from_secs(2))
            .connect(server.url())
            .await
            .expect("connect to NATS testcontainer");
        let context = jetstream::new(client);
        let message = jetstream::Message {
            message: async_nats::Message {
                subject: "scheduler.test".into(),
                reply: Some(js_ack_reply(delivered).into()),
                payload: Bytes::new(),
                headers: None,
                status: None,
                description: None,
                length: 0,
            },
            context,
        };
        (server, message)
    }

    #[test]
    fn retry_delay_backs_off_exponentially_and_caps() {
        assert_eq!(retry_delay(0), Duration::from_secs(1));
        assert_eq!(retry_delay(1), Duration::from_secs(1));
        assert_eq!(retry_delay(2), Duration::from_secs(2));
        assert_eq!(retry_delay(3), Duration::from_secs(4));
        assert_eq!(retry_delay(6), Duration::from_secs(30));
        assert_eq!(retry_delay(i64::MAX), Duration::from_secs(30));
    }

    fn execution_stream(allow_message_schedules: bool) -> jetstream::stream::Config {
        jetstream::stream::Config {
            name: SCHEDULE_EXECUTION_STREAM.to_string(),
            allow_message_schedules,
            ..Default::default()
        }
    }

    #[test]
    fn scheduling_support_requires_nats_2_14_or_newer() {
        let stream = execution_stream(true);

        assert!(verify_message_scheduling_support("2.14.0", &stream).is_ok());
        assert!(verify_message_scheduling_support("2.15.1-beta.2", &stream).is_ok());
        assert!(verify_message_scheduling_support("3.0.0", &stream).is_ok());

        // 2.12/2.13 only deliver `@at` schedules; cron and `@every` would
        // publish successfully and never fire.
        assert!(matches!(
            verify_message_scheduling_support("2.12.0", &stream),
            Err(SchedulingSupportError::ServerTooOld { version }) if version == "2.12.0"
        ));
        assert!(matches!(
            verify_message_scheduling_support("2.13.1", &stream),
            Err(SchedulingSupportError::ServerTooOld { .. })
        ));
        assert!(matches!(
            verify_message_scheduling_support("1.4.0", &stream),
            Err(SchedulingSupportError::ServerTooOld { .. })
        ));
    }

    #[test]
    fn scheduling_support_rejects_unrecognized_server_versions() {
        let stream = execution_stream(true);

        for version in ["", "nats", "2", "v2.12.0"] {
            assert!(matches!(
                verify_message_scheduling_support(version, &stream),
                Err(SchedulingSupportError::UnrecognizedServerVersion { .. })
            ));
        }
    }

    #[test]
    fn scheduling_support_requires_allow_message_schedules_on_the_stream() {
        let error = verify_message_scheduling_support("2.14.0", &execution_stream(false)).unwrap_err();

        assert!(matches!(
            error,
            SchedulingSupportError::SchedulesNotAllowed { ref stream } if stream == SCHEDULE_EXECUTION_STREAM
        ));
        assert!(error.to_string().contains("allow_message_schedules"));
    }

    #[test]
    fn event_filter_is_derived_from_the_event_subject_prefix() {
        use crate::processor::execution::reconciliation::EVENT_SUBJECT_PREFIX;

        assert_eq!(SCHEDULE_EVENT_FILTER, format!("{EVENT_SUBJECT_PREFIX}.>"));
    }

    #[test]
    fn consumer_config_matches_the_scheduler_contract() {
        let config = scheduler_execution_consumer_config();

        assert_eq!(config.durable_name.as_deref(), Some("scheduler_execution_v1"));
        assert!(matches!(config.deliver_policy, DeliverPolicy::All));
        assert!(matches!(config.ack_policy, AckPolicy::Explicit));
        assert!(matches!(config.replay_policy, ReplayPolicy::Instant));
        assert_eq!(config.ack_wait, Duration::from_secs(120));
        assert_eq!(config.max_deliver, -1);
        assert_eq!(config.filter_subject, "scheduler.schedules.events.v1.>");
        // max_ack_pending bounds concurrency but is intentionally not 1: global
        // serialization is not wanted.
        assert_eq!(config.max_ack_pending, 256);
        assert_ne!(config.max_ack_pending, 1);
        assert!(config.backoff.is_empty());
    }

    #[tokio::test]
    async fn jetstream_delivered_message_reports_redelivery() {
        let (_first_server, first_message) = jetstream_message(1).await;
        let (_again_server, again_message) = jetstream_message(2).await;
        let first = JetStreamDeliveredMessage::new(first_message);
        let again = JetStreamDeliveredMessage::new(again_message);

        assert!(!DeliveredMessage::is_redelivery(&first));
        assert!(DeliveredMessage::is_redelivery(&again));
    }

    #[tokio::test]
    async fn jetstream_delivered_message_settlement_methods_run() {
        let (_server, message) = jetstream_message(1).await;
        let delivered = JetStreamDeliveredMessage::new(message);
        DeliveredMessage::ack(&delivered).await.unwrap();
        DeliveredMessage::term(&delivered).await.unwrap();
        DeliveredMessage::retry(&delivered).await.unwrap();

        let inner = delivered.into_inner();
        assert_eq!(inner.subject.as_str(), "scheduler.test");
    }

    #[tokio::test]
    async fn jetstream_delivered_message_new_and_into_inner_round_trip() {
        let (_server, message) = jetstream_message(1).await;
        let wrapped = JetStreamDeliveredMessage::new(message);
        let inner = wrapped.into_inner();
        assert_eq!(inner.subject.as_str(), "scheduler.test");
    }
