use super::*;

    #[test]
    fn test_reconnect_delay_starts_at_one_second() {
        assert_eq!(reconnect_delay(0).as_secs(), 1);
    }

    #[test]
    fn test_reconnect_delay_exponential_backoff() {
        assert_eq!(reconnect_delay(0).as_secs(), 1);
        assert_eq!(reconnect_delay(1).as_secs(), 2);
        assert_eq!(reconnect_delay(2).as_secs(), 4);
        assert_eq!(reconnect_delay(3).as_secs(), 8);
        assert_eq!(reconnect_delay(4).as_secs(), 16);
    }

    #[test]
    fn test_reconnect_delay_caps_at_max() {
        assert_eq!(reconnect_delay(5).as_secs(), 30);
        assert_eq!(reconnect_delay(10).as_secs(), 30);
        assert_eq!(reconnect_delay(100).as_secs(), 30);
    }

    #[test]
    fn test_reconnect_delay_overflow_protection() {
        assert_eq!(reconnect_delay(usize::MAX).as_secs(), 30);
    }

    #[tokio::test]
    async fn test_handle_event_connected() {
        handle_event(Event::Connected).await;
    }

    #[tokio::test]
    async fn test_handle_event_disconnected() {
        handle_event(Event::Disconnected).await;
    }

    #[tokio::test]
    async fn test_handle_event_all_variants() {
        use async_nats::{ClientError, ServerError};

        handle_event(Event::Connected).await;
        handle_event(Event::Disconnected).await;
        handle_event(Event::ServerError(ServerError::Other("test".to_string()))).await;
        handle_event(Event::ClientError(ClientError::Other("test".to_string()))).await;
        handle_event(Event::SlowConsumer(42)).await;
        handle_event(Event::LameDuckMode).await;
        handle_event(Event::Closed).await;
        handle_event(Event::Draining).await;
    }

    #[test]
    fn test_max_reconnect_delay_constant() {
        assert_eq!(MAX_RECONNECT_DELAY.as_secs(), 30);
    }

    #[test]
    fn connect_error_display_invalid_credentials() {
        let err = ConnectError::InvalidCredentials(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"));
        let msg = err.to_string();
        assert!(msg.contains("Failed to load credentials file"));
        assert!(msg.contains("file not found"));
    }

    #[test]
    fn connect_error_source_invalid_credentials() {
        let err = ConnectError::InvalidCredentials(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"));
        assert!(std::error::Error::source(&err).is_some());
    }

    #[tokio::test]
    async fn connect_with_missing_credentials_file_returns_invalid_credentials() {
        let config = NatsConfig::new(
            vec!["nats://127.0.0.1:4222".to_string()],
            NatsAuth::Credentials("/nonexistent/path/to/creds.nk".into()),
        );

        let err = connect(&config, Duration::from_millis(100))
            .await
            .expect_err("expected an error for missing credentials file");

        assert!(matches!(err, ConnectError::InvalidCredentials(_)));
    }
