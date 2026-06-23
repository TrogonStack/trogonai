use super::*;
    use std::error::Error;

    fn valid_test_secret() -> String {
        ["whsec_", "dGVzdC1zZWNyZXQ="].concat()
    }

    #[test]
    fn accepts_prefixed_secret() {
        let secret = IncidentioSigningSecret::new(valid_test_secret()).unwrap();
        assert_eq!(secret.as_bytes(), b"test-secret");
    }

    #[test]
    fn rejects_bare_secret() {
        let err = IncidentioSigningSecret::new("dGVzdC1zZWNyZXQ=").unwrap_err();
        assert_eq!(err.to_string(), "signing secret must start with whsec_");
    }

    #[test]
    fn rejects_empty_secret() {
        let err = IncidentioSigningSecret::new("").unwrap_err();
        assert_eq!(err.to_string(), "signing secret must not be empty");
        assert!(err.source().is_none());
    }

    #[test]
    fn rejects_empty_prefixed_secret() {
        let err = IncidentioSigningSecret::new("whsec_").unwrap_err();
        assert_eq!(err.to_string(), "signing secret must not be empty");
    }

    #[test]
    fn rejects_invalid_base64() {
        let err = IncidentioSigningSecret::new("whsec_not-base64!").unwrap_err();
        assert_eq!(err.to_string(), "signing secret must be valid base64");
        assert!(err.source().is_some());
    }

    #[test]
    fn missing_prefix_has_no_source() {
        let err = IncidentioSigningSecret::new("dGVzdA==").unwrap_err();
        assert_eq!(err.to_string(), "signing secret must start with whsec_");
        assert!(err.source().is_none());
    }

    #[test]
    fn debug_redacts() {
        let secret = IncidentioSigningSecret::new(valid_test_secret()).unwrap();
        assert_eq!(format!("{secret:?}"), "IncidentioSigningSecret(****)");
    }
