use super::*;

    #[test]
    fn acp_prefix_new_valid() {
        let p = AcpPrefix::new("acp").unwrap();
        assert_eq!(p.as_str(), "acp");
        assert_eq!(AcpPrefix::new("my.multi.part").unwrap().as_str(), "my.multi.part");
    }

    #[test]
    fn acp_prefix_new_invalid_returns_err() {
        assert!(AcpPrefix::new("").is_err());
        assert!(AcpPrefix::new("acp.*").is_err());
        assert!(AcpPrefix::new("acp.>").is_err());
        assert!(AcpPrefix::new("acp prefix").is_err());
        assert!(AcpPrefix::new("acp\t").is_err());
        assert!(AcpPrefix::new("acp\n").is_err());
        assert!(AcpPrefix::new("acp..foo").is_err());
        assert!(AcpPrefix::new(".acp").is_err());
        assert!(AcpPrefix::new("acp.").is_err());
        assert!(AcpPrefix::new("a".repeat(129)).is_err());
    }

    #[test]
    fn acp_prefix_new_validates_direct() {
        assert!(AcpPrefix::new("acp").is_ok());
        assert!(AcpPrefix::new("a").is_ok());
        assert!(AcpPrefix::new("my.multi.part").is_ok());
        assert!(AcpPrefix::new("a".repeat(128)).is_ok());
        assert!(matches!(AcpPrefix::new(""), Err(AcpPrefixError::Empty)));
        assert!(matches!(
            AcpPrefix::new("a".repeat(129)),
            Err(AcpPrefixError::TooLong(129))
        ));
    }

    #[test]
    fn acp_prefix_error_display() {
        assert_eq!(format!("{}", AcpPrefixError::Empty), "acp_prefix must not be empty");
        assert_eq!(
            format!("{}", AcpPrefixError::InvalidCharacter('*')),
            "acp_prefix contains invalid character: '*'"
        );
        assert_eq!(
            format!("{}", AcpPrefixError::TooLong(200)),
            "acp_prefix is too long: 200 bytes (max 128)"
        );
    }
