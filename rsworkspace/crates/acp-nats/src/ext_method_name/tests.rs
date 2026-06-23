use super::*;

    #[test]
    fn ext_method_name_valid() {
        assert!(ExtMethodName::new("my_custom_method").is_ok());
        assert!(ExtMethodName::new("simple").is_ok());
        assert!(ExtMethodName::new("method123").is_ok());
    }

    #[test]
    fn ext_method_name_too_long_returns_err() {
        let long = "a".repeat(129);
        let err = ExtMethodName::new(&long).err().unwrap();
        assert_eq!(err, ExtMethodNameError::TooLong(129));
    }

    #[test]
    fn ext_method_name_dotted_namespaces_accepted() {
        assert!(ExtMethodName::new("my.custom.method").is_ok());
        assert!(ExtMethodName::new("a.b").is_ok());
        assert!(ExtMethodName::new("vendor.operation").is_ok());
    }

    #[test]
    fn ext_method_name_malformed_dots_rejected() {
        assert!(ExtMethodName::new("..method").is_err());
        assert!(ExtMethodName::new("method..name").is_err());
        assert!(ExtMethodName::new(".method").is_err());
        assert!(ExtMethodName::new("method.").is_err());
        assert!(ExtMethodName::new(".").is_err());
    }

    #[test]
    fn ext_method_name_empty_returns_err() {
        let err = ExtMethodName::new("").err().unwrap();
        assert_eq!(err, ExtMethodNameError::Empty);
    }

    #[test]
    fn ext_method_name_wildcard_returns_err() {
        assert!(ExtMethodName::new("method.*").is_err());
        assert!(ExtMethodName::new("method.>").is_err());
    }

    #[test]
    fn ext_method_name_whitespace_returns_err() {
        assert!(ExtMethodName::new("method name").is_err());
        assert!(ExtMethodName::new("method\t").is_err());
    }

    #[test]
    fn ext_method_name_display_and_deref() {
        let name = ExtMethodName::new("my_method").unwrap();
        assert_eq!(format!("{}", name), "my_method");
        assert_eq!(name.len(), 9);
        assert!(name.starts_with("my"));
    }

    #[test]
    fn ext_method_name_error_display() {
        assert_eq!(format!("{}", ExtMethodNameError::Empty), "method must not be empty");
        assert_eq!(
            format!("{}", ExtMethodNameError::InvalidCharacter(' ')),
            "method contains invalid character: ' '"
        );
        assert_eq!(
            format!("{}", ExtMethodNameError::TooLong(200)),
            "method is too long: 200 bytes (max 128)"
        );
    }
