    use trogon_nats::jetstream::mocks::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

    use super::*;

    fn test_prefix() -> A2aPrefix {
        A2aPrefix::new("a2a".to_string()).unwrap()
    }

    fn test_task_id() -> A2aTaskId {
        A2aTaskId::new("task-resub-1").unwrap()
    }

    #[tokio::test]
    async fn open_resubscribe_stream_succeeds_with_valid_consumer() {
        let js = MockJetStreamConsumerFactory::new();
        let (consumer, _tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);

        let stream = open_resubscribe_stream(&js, &test_prefix(), &test_task_id(), 42).await;
        assert!(stream.is_ok());
    }

    #[tokio::test]
    async fn returned_stream_has_last_seq_initialized_to_provided_value() {
        let js = MockJetStreamConsumerFactory::new();
        let (consumer, _tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);

        let stream = open_resubscribe_stream(&js, &test_prefix(), &test_task_id(), 99)
            .await
            .unwrap();
        assert_eq!(stream.last_seq(), 99);
    }

    #[tokio::test]
    async fn get_stream_failure_returns_consumer_setup_error() {
        let js = MockJetStreamConsumerFactory::new();
        js.fail_get_stream_at(1);

        let result = open_resubscribe_stream(&js, &test_prefix(), &test_task_id(), 0).await;
        assert!(matches!(result, Err(ClientError::ConsumerSetup(_))));
    }

    #[tokio::test]
    async fn no_available_consumer_returns_consumer_setup_error() {
        let js = MockJetStreamConsumerFactory::new();

        let result = open_resubscribe_stream(&js, &test_prefix(), &test_task_id(), 0).await;
        assert!(matches!(result, Err(ClientError::ConsumerSetup(_))));
    }

    #[tokio::test]
    async fn last_seq_zero_succeeds() {
        let js = MockJetStreamConsumerFactory::new();
        let (consumer, _tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);

        let stream = open_resubscribe_stream(&js, &test_prefix(), &test_task_id(), 0).await;
        assert!(stream.is_ok());
        assert_eq!(stream.unwrap().last_seq(), 0);
    }
