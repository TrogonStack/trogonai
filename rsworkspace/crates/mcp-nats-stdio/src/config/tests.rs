use super::*;
    use trogon_std::FixedArgs;
    use trogon_std::env::InMemoryEnv;

    fn args() -> FixedArgs<Args> {
        FixedArgs(Args {
            mcp_prefix: None,
            client_id: None,
            server_id: None,
        })
    }

    fn config_from_env(env: &InMemoryEnv) -> BridgeConfig {
        let config = base_config(&args(), env).unwrap();
        BridgeConfig {
            mcp: mcp_nats::apply_timeout_overrides(config.mcp, env),
            client_id: config.client_id,
            server_id: config.server_id,
        }
    }

    fn err(result: Result<BridgeConfig, ConfigError>) -> ConfigError {
        match result {
            Ok(_) => panic!("expected invalid MCP stdio bridge config"),
            Err(error) => error,
        }
    }

    #[test]
    fn default_config_uses_mcp_prefix_and_stdio_peers() {
        let env = InMemoryEnv::new();
        let config = config_from_env(&env);
        assert_eq!(config.mcp.prefix_str(), "mcp");
        assert_eq!(config.client_id.as_str(), "stdio");
        assert_eq!(config.server_id.as_str(), "default");
        assert_eq!(config.mcp.nats().servers, vec!["localhost:4222"]);
        assert!(matches!(&config.mcp.nats().auth, mcp_nats::NatsAuth::None));
    }

    #[test]
    fn reads_prefix_and_peer_ids_from_env_provider() {
        let env = InMemoryEnv::new();
        env.set("MCP_PREFIX", "tenant.mcp");
        env.set("MCP_CLIENT_ID", "desktop");
        env.set("MCP_SERVER_ID", "filesystem");
        let config = config_from_env(&env);
        assert_eq!(config.mcp.prefix_str(), "tenant.mcp");
        assert_eq!(config.client_id.as_str(), "desktop");
        assert_eq!(config.server_id.as_str(), "filesystem");
    }

    #[test]
    fn args_override_env_provider() {
        let env = InMemoryEnv::new();
        env.set("MCP_PREFIX", "env-mcp");
        env.set("MCP_CLIENT_ID", "env-client");
        env.set("MCP_SERVER_ID", "env-server");
        let parser = FixedArgs(Args {
            mcp_prefix: Some("cli-mcp".to_string()),
            client_id: Some("cli-client".to_string()),
            server_id: Some("cli-server".to_string()),
        });

        let config = base_config(&parser, &env).unwrap();

        assert_eq!(config.mcp.prefix_str(), "cli-mcp");
        assert_eq!(config.client_id.as_str(), "cli-client");
        assert_eq!(config.server_id.as_str(), "cli-server");
    }

    #[test]
    fn reads_nats_config_from_env_provider() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "host1:4222,host2:4222");
        env.set("NATS_TOKEN", "my-token");

        let config = config_from_env(&env);

        assert_eq!(config.mcp.nats().servers, vec!["host1:4222", "host2:4222"]);
        assert!(matches!(&config.mcp.nats().auth, mcp_nats::NatsAuth::Token(token) if token == "my-token"));
    }

    #[test]
    fn rejects_invalid_client_id() {
        let env = InMemoryEnv::new();
        let parser = FixedArgs(Args {
            mcp_prefix: None,
            client_id: Some("bad.client".to_string()),
            server_id: None,
        });

        assert!(matches!(base_config(&parser, &env), Err(ConfigError::ClientId(_))));
    }

    #[test]
    fn rejects_invalid_prefix() {
        let env = InMemoryEnv::new();
        let parser = FixedArgs(Args {
            mcp_prefix: Some("bad.*".to_string()),
            client_id: None,
            server_id: None,
        });

        assert!(matches!(base_config(&parser, &env), Err(ConfigError::Prefix(_))));
    }

    #[test]
    fn rejects_invalid_server_id() {
        let env = InMemoryEnv::new();
        let parser = FixedArgs(Args {
            mcp_prefix: None,
            client_id: None,
            server_id: Some("bad.server".to_string()),
        });

        assert!(matches!(base_config(&parser, &env), Err(ConfigError::ServerId(_))));
    }

    #[test]
    fn config_error_display_and_source_are_specific() {
        let env = InMemoryEnv::new();

        let prefix = err(base_config(
            &FixedArgs(Args {
                mcp_prefix: Some("bad.*".to_string()),
                client_id: None,
                server_id: None,
            }),
            &env,
        ));
        assert_eq!(prefix.to_string(), "invalid MCP prefix");
        assert!(std::error::Error::source(&prefix).is_some());

        let client = err(base_config(
            &FixedArgs(Args {
                mcp_prefix: None,
                client_id: Some("bad.client".to_string()),
                server_id: None,
            }),
            &env,
        ));
        assert_eq!(client.to_string(), "invalid MCP client id");
        assert!(std::error::Error::source(&client).is_some());

        let server = err(base_config(
            &FixedArgs(Args {
                mcp_prefix: None,
                client_id: None,
                server_id: Some("bad.server".to_string()),
            }),
            &env,
        ));
        assert_eq!(server.to_string(), "invalid MCP server id");
        assert!(std::error::Error::source(&server).is_some());
    }

    #[test]
    #[should_panic(expected = "expected invalid MCP stdio bridge config")]
    fn err_panics_when_config_is_valid() {
        let env = InMemoryEnv::new();

        let _ = err(base_config(&args(), &env));
    }
