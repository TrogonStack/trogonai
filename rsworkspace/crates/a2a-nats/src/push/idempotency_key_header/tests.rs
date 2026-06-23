    use super::*;

    #[test]
    fn try_from_accepts_valid_header_name() {
        let header = IdempotencyKeyHeader::try_from("Idempotency-Key").unwrap();
        assert_eq!(header.as_http().as_str(), "idempotency-key");
        assert_eq!(header.to_string(), "idempotency-key");
    }

    #[test]
    fn try_from_trims_whitespace_before_parsing() {
        let header = IdempotencyKeyHeader::try_from("  X-Push-Key  ").unwrap();
        assert_eq!(header.as_http().as_str(), "x-push-key");
    }

    #[test]
    fn try_from_rejects_invalid_token() {
        let err = IdempotencyKeyHeader::try_from("not a valid header").unwrap_err();
        assert!(matches!(err, IdempotencyKeyHeaderError::InvalidToken));
    }

    #[test]
    fn error_display_describes_invalid_token() {
        let err = IdempotencyKeyHeaderError::InvalidToken;
        assert!(err.to_string().contains("invalid HTTP header name"));
        assert!(format!("{err:?}").contains("InvalidToken"));
    }

    #[test]
    fn new_wraps_existing_header_name() {
        let name = HeaderName::from_static("x-trace");
        let header = IdempotencyKeyHeader::new(name);
        assert_eq!(header.as_http().as_str(), "x-trace");
    }

    #[test]
    fn debug_shows_wrapped_header_value() {
        let header = IdempotencyKeyHeader::try_from("Idempotency-Key").unwrap();
        assert!(format!("{header:?}").contains("idempotency-key"));
    }
