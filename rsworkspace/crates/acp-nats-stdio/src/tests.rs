use super::*;
    use trogon_nats::AdvancedMockNatsClient;

    #[derive(Clone)]
    struct MockJs {
        publisher: trogon_nats::jetstream::MockJetStreamPublisher,
        consumer_factory: trogon_nats::jetstream::MockJetStreamConsumerFactory,
    }

    impl MockJs {
        fn new() -> Self {
            Self {
                publisher: trogon_nats::jetstream::MockJetStreamPublisher::new(),
                consumer_factory: trogon_nats::jetstream::MockJetStreamConsumerFactory::new(),
            }
        }
    }

    impl trogon_nats::jetstream::JetStreamPublisher for MockJs {
        type PublishError = trogon_nats::mocks::MockError;
        type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

        async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
            &self,
            subject: S,
            headers: async_nats::HeaderMap,
            payload: bytes::Bytes,
        ) -> Result<Self::AckFuture, Self::PublishError> {
            self.publisher.publish_with_headers(subject, headers, payload).await
        }
    }

    impl trogon_nats::jetstream::JetStreamGetStream for MockJs {
        type Error = async_nats::jetstream::context::GetStreamError;
        type Stream = trogon_nats::jetstream::MockJetStreamStream;

        async fn get_stream<T: AsRef<str> + Send>(
            &self,
            stream_name: T,
        ) -> Result<trogon_nats::jetstream::MockJetStreamStream, Self::Error> {
            self.consumer_factory.get_stream(stream_name).await
        }
    }

    #[tokio::test]
    async fn run_bridge_shuts_down_on_signal() {
        let mock = AdvancedMockNatsClient::new();
        let _sub = mock.inject_messages();
        let config = acp_nats::Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        );

        let (reader, _writer) = tokio::io::duplex(1024);
        let (_reader2, writer2) = tokio::io::duplex(1024);
        let stdin = async_compat::Compat::new(reader);
        let stdout = async_compat::Compat::new(writer2);

        let local = tokio::task::LocalSet::new();
        let result = local
            .run_until(run_bridge(
                mock,
                MockJs::new(),
                &config,
                stdout,
                stdin,
                std::future::ready(()),
            ))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_bridge_exits_on_io_close() {
        let mock = AdvancedMockNatsClient::new();
        let _sub = mock.inject_messages();
        let config = acp_nats::Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        );

        let (reader, writer) = tokio::io::duplex(1024);
        let (_reader2, writer2) = tokio::io::duplex(1024);
        drop(writer);
        let stdin = async_compat::Compat::new(reader);
        let stdout = async_compat::Compat::new(writer2);

        let local = tokio::task::LocalSet::new();
        let result = local
            .run_until(run_bridge(
                mock,
                MockJs::new(),
                &config,
                stdout,
                stdin,
                std::future::pending(),
            ))
            .await;

        assert!(result.is_ok());
    }
