use super::*;

    #[test]
    fn client_state_roundtrips() {
        let client_state = MicrosoftGraphClientState::new("super-secret").unwrap();
        assert_eq!(client_state.as_str(), "super-secret");
    }

    #[test]
    fn client_state_debug_redacts() {
        let client_state = MicrosoftGraphClientState::new("super-secret").unwrap();
        assert_eq!(format!("{client_state:?}"), "MicrosoftGraphClientState(****)");
    }

    #[test]
    fn matches_equal_client_state() {
        let client_state = MicrosoftGraphClientState::new("super-secret").unwrap();
        assert!(client_state.matches("super-secret"));
    }

    #[test]
    fn rejects_different_client_state() {
        let client_state = MicrosoftGraphClientState::new("super-secret").unwrap();
        assert!(!client_state.matches("wrong-secret"));
    }
