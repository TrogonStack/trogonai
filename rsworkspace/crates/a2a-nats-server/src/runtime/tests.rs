use trogon_std::env::InMemoryEnv;

    use super::*;

    #[test]
    fn runtime_error_display_missing_agent_id() {
        assert_eq!(
            RuntimeError::MissingAgentId.to_string(),
            "A2A_AGENT_ID env var is required but not set"
        );
    }

    #[test]
    fn runtime_error_display_and_source_for_invalid_agent_id() {
        let inner = A2aAgentId::new("a.b").unwrap_err();
        let e = RuntimeError::InvalidAgentId(inner);
        assert_eq!(e.to_string(), "invalid agent id");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn runtime_error_display_and_source_for_invalid_prefix() {
        let inner = A2aPrefix::new("").unwrap_err();
        let e = RuntimeError::InvalidPrefix(inner);
        assert_eq!(e.to_string(), "invalid A2A prefix");
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
        env.set(ENV_A2A_AGENT_ID, "bot");
        env.set(ENV_A2A_PREFIX, "bad prefix!");
        let result = parse_env(&env);
        assert!(matches!(result, Err(RuntimeError::InvalidPrefix(_))));
    }

    #[test]
    fn runtime_error_display_and_source_for_nats_connect() {
        let inner = trogon_nats::ConnectError::InvalidCredentials(std::io::Error::other("oops"));
        let e = RuntimeError::NatsConnect(inner);
        assert_eq!(e.to_string(), "NATS connection failed");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn runtime_error_display_and_source_for_provision() {
        let inner = a2a_nats::jetstream::ProvisionError("stream create failed".to_string());
        let e = RuntimeError::Provision(inner);
        assert_eq!(e.to_string(), "JetStream provisioning failed");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn runtime_error_display_and_source_for_bridge() {
        let inner = a2a_nats::server::BridgeError::Subscribe(Box::new(std::io::Error::other("denied")));
        let e = RuntimeError::Bridge(inner);
        assert_eq!(e.to_string(), "bridge error");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn parse_env_valid_sets_default_prefix() {
        let env = InMemoryEnv::new();
        env.set(ENV_A2A_AGENT_ID, "bot");
        let cfg = parse_env(&env).expect("valid env");
        assert_eq!(cfg.prefix.as_str(), DEFAULT_A2A_PREFIX);
        assert_eq!(cfg.agent_id.as_str(), "bot");
    }
