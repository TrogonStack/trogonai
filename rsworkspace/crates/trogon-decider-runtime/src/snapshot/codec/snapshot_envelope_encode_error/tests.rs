use super::*;

    #[test]
    fn display_and_source_preserve_envelope_encode_error() {
        let source = serde_json::from_slice::<serde_json::Value>(b"not-json").unwrap_err();
        let source_message = source.to_string();
        let error = SnapshotEnvelopeEncodeError::new(source);

        assert_eq!(
            error.to_string(),
            format!("failed to encode snapshot envelope: {source_message}")
        );
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some(source_message)
        );
    }
