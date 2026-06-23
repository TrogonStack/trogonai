use super::*;

    fn stream_exists_error() -> CreateStreamError {
        let source: async_nats::jetstream::Error = serde_json::from_str(
            r#"{"code":400,"err_code":10058,"description":"stream name already in use with a different configuration"}"#,
        )
        .unwrap();

        CreateStreamError::new(CreateStreamErrorKind::JetStream(source))
    }

    #[test]
    fn key_value_already_exists_matches_wrapped_stream_exists_error() {
        let error = CreateKeyValueError::with_source(CreateKeyValueErrorKind::BucketCreate, stream_exists_error());

        assert!(is_create_key_value_already_exists(&error));
    }
