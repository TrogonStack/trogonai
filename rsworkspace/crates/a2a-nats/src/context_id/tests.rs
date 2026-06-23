    use super::*;

    #[test]
    fn a2a_context_id_valid() {
        assert!(A2aContextId::new("ctx-1").is_ok());
    }

    #[test]
    fn a2a_context_id_rejects_invalid() {
        assert!(A2aContextId::new("").is_err());
        assert!(A2aContextId::new("a.b").is_err());
        assert!(A2aContextId::new("a*").is_err());
    }

    #[test]
    fn a2a_context_id_generate_unique() {
        let a = A2aContextId::generate();
        let b = A2aContextId::generate();
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn a2a_context_id_display_matches_inner() {
        let id = A2aContextId::new("ctx-display").unwrap();
        assert_eq!(format!("{id}"), "ctx-display");
    }

    #[test]
    fn a2a_context_id_derefs_to_str() {
        let id = A2aContextId::new("ctx-deref").unwrap();
        let s: &str = &id;
        assert_eq!(s, "ctx-deref");
    }

    #[test]
    fn context_id_error_display() {
        assert_eq!(ContextIdError::Empty.to_string(), "context_id must not be empty");
        assert_eq!(
            ContextIdError::InvalidCharacter('.').to_string(),
            "context_id contains invalid character: '.'"
        );
        assert_eq!(
            ContextIdError::TooLong(200).to_string(),
            "context_id is too long: 200 characters (max 128)"
        );
    }
