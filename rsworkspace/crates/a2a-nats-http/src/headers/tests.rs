    use super::*;

    #[test]
    fn parses_optional_extension_with_question_prefix() {
        let ext = RequestedExtension::parse("?https://example.com/ext/foo").unwrap();
        assert_eq!(ext.uri, "https://example.com/ext/foo");
        assert!(!ext.required);
    }

    #[test]
    fn parses_required_extension_default() {
        let ext = RequestedExtension::parse("https://example.com/ext/bar").unwrap();
        assert_eq!(ext.uri, "https://example.com/ext/bar");
        assert!(ext.required);
    }

    #[test]
    fn parses_q_zero_as_optional() {
        let ext = RequestedExtension::parse("https://example.com/ext/baz;q=0").unwrap();
        assert_eq!(ext.uri, "https://example.com/ext/baz");
        assert!(!ext.required);
    }

    #[test]
    fn parse_extensions_splits_on_comma_and_ignores_empty() {
        let exts = parse_extensions("https://a/, ,?https://b/");
        assert_eq!(exts.len(), 2);
        assert_eq!(exts[0].uri, "https://a/");
        assert!(exts[0].required);
        assert_eq!(exts[1].uri, "https://b/");
        assert!(!exts[1].required);
    }

    #[test]
    fn default_config_has_one_version() {
        let cfg = SpecNegotiationConfig::default();
        assert!(cfg.supported_versions.contains(DEFAULT_A2A_VERSION));
        assert_eq!(cfg.default_version, DEFAULT_A2A_VERSION);
        assert!(cfg.supported_extensions.is_empty());
    }

    #[test]
    fn with_version_appends_to_supported_set() {
        let cfg = SpecNegotiationConfig::new("0.3.0").with_version("0.2.0");
        assert!(cfg.supported_versions.contains("0.3.0"));
        assert!(cfg.supported_versions.contains("0.2.0"));
    }

    #[test]
    fn with_extension_appends() {
        let cfg = SpecNegotiationConfig::default().with_extension("https://example.com/ext/x");
        assert!(cfg.supported_extensions.contains("https://example.com/ext/x"));
    }
