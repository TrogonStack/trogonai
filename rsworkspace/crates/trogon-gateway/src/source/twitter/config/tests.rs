use super::*;

    #[test]
    fn consumer_secret_roundtrips() {
        let secret = TwitterConsumerSecret::new("super-secret").unwrap();
        assert_eq!(secret.as_str(), "super-secret");
    }

    #[test]
    fn consumer_secret_debug_redacts() {
        let secret = TwitterConsumerSecret::new("super-secret").unwrap();
        assert_eq!(format!("{secret:?}"), "TwitterConsumerSecret(****)");
    }
