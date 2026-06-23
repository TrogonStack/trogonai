use super::*;

    #[test]
    fn single_valid() {
        assert!(NatsToken::new("valid-session-123").is_ok());
        assert!(NatsToken::new("a").is_ok());
        assert_eq!(NatsToken::new("hello").unwrap().as_str(), "hello");
    }

    #[test]
    fn single_empty() {
        assert_eq!(NatsToken::new(""), Err(SubjectTokenViolation::Empty));
    }

    #[test]
    fn single_too_long() {
        let long = "a".repeat(129);
        assert_eq!(NatsToken::new(&long), Err(SubjectTokenViolation::TooLong(129)));
        assert!(NatsToken::new("a".repeat(128)).is_ok());
    }

    #[test]
    fn single_rejects_dots() {
        assert_eq!(NatsToken::new("a.b"), Err(SubjectTokenViolation::InvalidCharacter('.')));
    }

    #[test]
    fn single_rejects_wildcards() {
        assert!(NatsToken::new("a*").is_err());
        assert!(NatsToken::new("a>").is_err());
        assert!(NatsToken::new(">").is_err());
    }

    #[test]
    fn single_rejects_whitespace() {
        assert!(NatsToken::new("a b").is_err());
        assert!(NatsToken::new("a\t").is_err());
        assert!(NatsToken::new("a\n").is_err());
    }

    #[test]
    fn single_rejects_non_ascii() {
        assert_eq!(
            NatsToken::new("séssion"),
            Err(SubjectTokenViolation::InvalidCharacter('é'))
        );
    }

    #[test]
    fn dotted_valid_simple() {
        assert!(DottedNatsToken::new("acp").is_ok());
        assert!(DottedNatsToken::new("a").is_ok());
    }

    #[test]
    fn dotted_valid_dotted() {
        assert_eq!(DottedNatsToken::new("my.multi.part").unwrap().as_str(), "my.multi.part");
        assert!(DottedNatsToken::new("a.b").is_ok());
        assert!(DottedNatsToken::new("vendor.operation").is_ok());
    }

    #[test]
    fn dotted_empty() {
        assert_eq!(DottedNatsToken::new(""), Err(SubjectTokenViolation::Empty));
    }

    #[test]
    fn dotted_too_long() {
        let long = "a".repeat(129);
        assert_eq!(DottedNatsToken::new(&long), Err(SubjectTokenViolation::TooLong(129)));
        assert!(DottedNatsToken::new("a".repeat(128)).is_ok());
    }

    #[test]
    fn dotted_rejects_wildcards() {
        assert!(DottedNatsToken::new("acp.*").is_err());
        assert!(DottedNatsToken::new("acp.>").is_err());
    }

    #[test]
    fn dotted_rejects_whitespace() {
        assert!(DottedNatsToken::new("acp prefix").is_err());
        assert!(DottedNatsToken::new("acp\t").is_err());
        assert!(DottedNatsToken::new("acp\n").is_err());
    }

    #[test]
    fn dotted_rejects_malformed_dots() {
        assert!(DottedNatsToken::new("..method").is_err());
        assert!(DottedNatsToken::new("method..name").is_err());
        assert!(DottedNatsToken::new(".method").is_err());
        assert!(DottedNatsToken::new("method.").is_err());
        assert!(DottedNatsToken::new(".").is_err());
        assert!(DottedNatsToken::new("acp..foo").is_err());
        assert!(DottedNatsToken::new(".acp").is_err());
        assert!(DottedNatsToken::new("acp.").is_err());
    }

    #[test]
    fn dotted_accepts_non_ascii() {
        assert!(DottedNatsToken::new("préfixe").is_ok());
    }

    #[test]
    fn single_display_and_deref() {
        let t = NatsToken::new("my-session").unwrap();
        assert_eq!(format!("{}", t), "my-session");
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("my"));
    }

    #[test]
    fn dotted_display_and_deref() {
        let t = DottedNatsToken::new("my.prefix").unwrap();
        assert_eq!(format!("{}", t), "my.prefix");
        assert_eq!(t.len(), 9);
        assert!(t.starts_with("my"));
    }

    #[test]
    fn single_as_ref_str() {
        let t = NatsToken::new("hello").unwrap();
        let s: &str = t.as_ref();
        assert_eq!(s, "hello");
    }

    #[test]
    fn dotted_as_ref_str() {
        let t = DottedNatsToken::new("hello").unwrap();
        let s: &str = t.as_ref();
        assert_eq!(s, "hello");
    }

    #[test]
    fn single_clone_and_eq() {
        let a = NatsToken::new("abc").unwrap();
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn dotted_clone_and_eq() {
        let a = DottedNatsToken::new("a.b.c").unwrap();
        let b = a.clone();
        assert_eq!(a, b);
    }
