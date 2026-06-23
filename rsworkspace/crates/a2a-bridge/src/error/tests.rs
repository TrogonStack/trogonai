    use std::error::Error;

    use super::*;

    #[test]
    fn display_mint() {
        assert!(
            BridgeError::Mint("unavailable".into())
                .to_string()
                .contains("auth callout mint")
        );
    }

    #[test]
    fn source_for_deserialize() {
        let e = BridgeError::Deserialize(serde_json::from_str::<serde_json::Value>("]").unwrap_err());
        assert!(e.source().is_some());
    }

    #[test]
    fn source_for_missing_authorization_none() {
        assert!(BridgeError::MissingAuthorization.source().is_none());
    }
