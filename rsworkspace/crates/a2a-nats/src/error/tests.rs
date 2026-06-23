    use super::*;

    #[test]
    fn codes_are_distinct() {
        let codes = [
            TASK_NOT_FOUND,
            TASK_NOT_CANCELABLE,
            PUSH_NOTIFICATION_NOT_SUPPORTED,
            UNSUPPORTED_OPERATION,
            CONTENT_TYPE_NOT_SUPPORTED,
            INVALID_AGENT_RESPONSE,
            EXTENDED_AGENT_CARD_NOT_CONFIGURED,
            EXTENSION_SUPPORT_REQUIRED,
            VERSION_NOT_SUPPORTED,
            AGENT_UNAVAILABLE,
        ];
        let mut seen = std::collections::HashSet::new();
        for code in codes {
            assert!(seen.insert(code), "duplicate error code {code}");
        }
    }

    #[test]
    fn codes_in_jsonrpc_server_range() {
        // JSON-RPC reserves -32000..-32099 for server errors. All our binding-specific codes
        // should fall in that range. Spec-defined codes are also there per A2A binding rules.
        for code in [
            TASK_NOT_FOUND,
            TASK_NOT_CANCELABLE,
            PUSH_NOTIFICATION_NOT_SUPPORTED,
            UNSUPPORTED_OPERATION,
            CONTENT_TYPE_NOT_SUPPORTED,
            INVALID_AGENT_RESPONSE,
            EXTENDED_AGENT_CARD_NOT_CONFIGURED,
            EXTENSION_SUPPORT_REQUIRED,
            VERSION_NOT_SUPPORTED,
            AGENT_UNAVAILABLE,
        ] {
            assert!((-32099..=-32000).contains(&code), "{code} out of server range");
        }
    }
