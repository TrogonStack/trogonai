use super::*;

    #[test]
    fn accepts_route_safe_ids() {
        let id = SourceIntegrationId::new("acme-main_1").unwrap();

        assert_eq!(id.as_str(), "acme-main_1");
        assert_eq!(id.stream_name_suffix(), "ACME-MAIN_1");
    }

    #[test]
    fn stream_name_suffix_preserves_hyphen_and_underscore_distinction() {
        let hyphen = SourceIntegrationId::new("acme-main").unwrap();
        let underscore = SourceIntegrationId::new("acme_main").unwrap();

        assert_eq!(hyphen.stream_name_suffix(), "ACME-MAIN");
        assert_eq!(underscore.stream_name_suffix(), "ACME_MAIN");
    }

    #[test]
    fn rejects_path_separators() {
        assert_eq!(
            SourceIntegrationId::new("acme/main"),
            Err(SourceIntegrationIdError::InvalidCharacter('/'))
        );
    }

    #[test]
    fn rejects_nats_unsafe_characters() {
        assert_eq!(
            SourceIntegrationId::new("acme.main"),
            Err(SourceIntegrationIdError::InvalidCharacter('.'))
        );
        assert_eq!(
            SourceIntegrationId::new("acme*main"),
            Err(SourceIntegrationIdError::InvalidCharacter('*'))
        );
    }

    #[test]
    fn rejects_too_long_ids_with_actual_length() {
        let value = "a".repeat(200);
        let err = SourceIntegrationId::new(&value).unwrap_err();

        assert_eq!(err, SourceIntegrationIdError::TooLong(200));
        assert_eq!(err.to_string(), "source integration id exceeds maximum length: 200");
    }
