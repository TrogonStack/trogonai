use trogon_std::env::InMemoryEnv;

    use super::*;

    #[test]
    fn runtime_error_display_missing_agent_id() {
        assert_eq!(
            RuntimeError::MissingAgentId.to_string(),
            "A2A_AGENT_ID environment variable is required"
        );
    }

    #[test]
    fn runtime_error_display_invalid_prefix() {
        let e = RuntimeError::InvalidPrefix(A2aPrefix::new("").unwrap_err());
        assert_eq!(e.to_string(), "invalid A2A prefix");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn runtime_error_display_invalid_agent_id() {
        let e = RuntimeError::InvalidAgentId(A2aAgentId::new("a.b").unwrap_err());
        assert_eq!(e.to_string(), "invalid agent id");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn runtime_error_display_and_source_for_nats_connect() {
        let inner = ConnectError::InvalidCredentials(std::io::Error::other("oops"));
        let e = RuntimeError::NatsConnect(inner);
        assert_eq!(e.to_string(), "NATS connection failed");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn runtime_error_missing_agent_id_has_no_source() {
        let e = RuntimeError::MissingAgentId;
        assert!(std::error::Error::source(&e).is_none());
    }

    #[test]
    fn parse_env_missing_agent_id_returns_error() {
        let env = InMemoryEnv::new();
        let result = parse_env(&env);
        assert!(matches!(result, Err(RuntimeError::MissingAgentId)));
    }

    #[test]
    fn parse_env_invalid_agent_id_returns_error() {
        let env = InMemoryEnv::new();
        env.set(ENV_A2A_AGENT_ID, "a.b");
        let result = parse_env(&env);
        assert!(matches!(result, Err(RuntimeError::InvalidAgentId(_))));
    }

    #[test]
    fn parse_env_invalid_prefix_returns_error() {
        let env = InMemoryEnv::new();
        env.set(ENV_A2A_PREFIX, "bad prefix!");
        env.set(ENV_A2A_AGENT_ID, "bot");
        let result = parse_env(&env);
        assert!(matches!(result, Err(RuntimeError::InvalidPrefix(_))));
    }

    #[test]
    fn parse_env_valid_uses_default_prefix() {
        let env = InMemoryEnv::new();
        env.set(ENV_A2A_AGENT_ID, "bot");
        let cfg = parse_env(&env).expect("valid");
        assert_eq!(cfg.prefix.as_str(), DEFAULT_A2A_PREFIX);
        assert_eq!(cfg.agent_id.as_str(), "bot");
    }

    #[tokio::test]
    #[cfg(coverage)]
    async fn coverage_run_stub_is_callable() {
        super::run().await.unwrap();
    }

    #[test]
    fn runtime_error_display_and_source_for_io_loop() {
        let e = RuntimeError::IoLoop(std::io::Error::other("nope"));
        assert_eq!(e.to_string(), "stdio loop failed");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn from_impls_construct_variants() {
        let e: RuntimeError = A2aPrefix::new("").unwrap_err().into();
        assert!(matches!(e, RuntimeError::InvalidPrefix(_)));
        let e: RuntimeError = A2aAgentId::new("a.b").unwrap_err().into();
        assert!(matches!(e, RuntimeError::InvalidAgentId(_)));
        let e: RuntimeError = ConnectError::InvalidCredentials(std::io::Error::other("x")).into();
        assert!(matches!(e, RuntimeError::NatsConnect(_)));
    }
