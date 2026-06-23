    use super::*;

    fn fresh_account_public() -> String {
        KeyPair::new_account().public_key()
    }

    #[test]
    fn parse_round_trips_through_as_str_display_debug() {
        let raw = fresh_account_public();
        let np = NkeyPublic::parse(raw.clone()).unwrap();
        assert_eq!(np.as_str(), raw);
        assert_eq!(np.to_string(), raw);
        assert!(format!("{np:?}").starts_with("NkeyPublic"));
    }

    #[test]
    fn parse_trims_surrounding_whitespace() {
        let raw = fresh_account_public();
        let padded = format!("  {raw}\n");
        assert_eq!(NkeyPublic::parse(padded).unwrap().as_str(), raw);
    }

    #[test]
    fn parse_rejects_empty_input() {
        let e = NkeyPublic::parse("   ").unwrap_err();
        assert!(matches!(e, AuthCalloutError::WireFormat(_)));
    }

    #[test]
    fn parse_rejects_garbage() {
        let e = NkeyPublic::parse("not-an-nkey").unwrap_err();
        assert!(matches!(e, AuthCalloutError::WireFormat(_)));
    }
