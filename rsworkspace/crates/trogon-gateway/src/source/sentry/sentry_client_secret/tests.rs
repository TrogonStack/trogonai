use super::*;

    #[test]
    fn sentry_client_secret_roundtrips() {
        let secret = SentryClientSecret::new("super-secret").unwrap();
        assert_eq!(secret.as_str(), "super-secret");
    }

    #[test]
    fn sentry_client_secret_debug_redacts() {
        let secret = SentryClientSecret::new("super-secret").unwrap();
        assert_eq!(format!("{secret:?}"), "SentryClientSecret(****)");
    }
