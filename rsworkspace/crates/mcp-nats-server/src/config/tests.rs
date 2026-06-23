use super::*;
    use crate::constants::MCP_ENDPOINT;
    use trogon_std::FixedArgs;
    use trogon_std::env::InMemoryEnv;

    fn args() -> FixedArgs<Args> {
        FixedArgs(Args {
            mcp_prefix: None,
            client_id_prefix: None,
            server_id: None,
            host: None,
            port: None,
            allowed_hosts: Vec::new(),
        })
    }

    fn config_from_env(env: &InMemoryEnv) -> HttpBridgeConfig {
        let config = base_config(&args(), env).unwrap();
        HttpBridgeConfig {
            mcp: mcp_nats::apply_timeout_overrides(config.mcp, env),
            client_id_prefix: config.client_id_prefix,
            server_id: config.server_id,
            bind_addr: config.bind_addr,
            allowed_hosts: config.allowed_hosts,
        }
    }

    fn err(result: Result<HttpBridgeConfig, ConfigError>) -> ConfigError {
        match result {
            Ok(_) => panic!("expected invalid MCP HTTP bridge config"),
            Err(error) => error,
        }
    }

    #[test]
    fn default_config_uses_mcp_prefix_http_peer_prefix_and_local_endpoint() {
        let config = config_from_env(&InMemoryEnv::new());

        assert_eq!(config.mcp.prefix_str(), "mcp");
        assert_eq!(config.client_id_prefix.as_str(), "http");
        assert_eq!(config.server_id.as_str(), "default");
        assert_eq!(config.bind_addr, "127.0.0.1:8081".parse().unwrap());
        assert_eq!(MCP_ENDPOINT, "/mcp");
        assert!(config.allowed_hosts.is_empty());
    }

    #[test]
    fn reads_config_from_env_provider() {
        let env = InMemoryEnv::new();
        env.set("MCP_PREFIX", "tenant.mcp");
        env.set("MCP_CLIENT_ID_PREFIX", "browser");
        env.set("MCP_SERVER_ID", "filesystem");
        env.set("MCP_HTTP_HOST", "0.0.0.0");
        env.set("MCP_HTTP_PORT", "9090");

        let config = config_from_env(&env);

        assert_eq!(config.mcp.prefix_str(), "tenant.mcp");
        assert_eq!(config.client_id_prefix.as_str(), "browser");
        assert_eq!(config.server_id.as_str(), "filesystem");
        assert_eq!(config.bind_addr, "0.0.0.0:9090".parse().unwrap());
    }

    #[test]
    fn args_override_env_provider() {
        let env = InMemoryEnv::new();
        env.set("MCP_PREFIX", "env-mcp");
        env.set("MCP_CLIENT_ID_PREFIX", "env-client");
        env.set("MCP_SERVER_ID", "env-server");
        env.set("MCP_HTTP_HOST", "127.0.0.1");
        env.set("MCP_HTTP_PORT", "9090");
        let parser = FixedArgs(Args {
            mcp_prefix: Some("cli-mcp".to_string()),
            client_id_prefix: Some("cli-client".to_string()),
            server_id: Some("cli-server".to_string()),
            host: Some("0.0.0.0".parse().unwrap()),
            port: Some(7070),
            allowed_hosts: vec!["example.com".to_string()],
        });

        let config = base_config(&parser, &env).unwrap();

        assert_eq!(config.mcp.prefix_str(), "cli-mcp");
        assert_eq!(config.client_id_prefix.as_str(), "cli-client");
        assert_eq!(config.server_id.as_str(), "cli-server");
        assert_eq!(config.bind_addr, "0.0.0.0:7070".parse().unwrap());
        assert_eq!(
            config.allowed_hosts.iter().map(AllowedHost::as_str).collect::<Vec<_>>(),
            vec!["example.com"]
        );
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
    fn rejects_invalid_values() {
        let env = InMemoryEnv::new();

        assert!(matches!(
            base_config(
                &FixedArgs(Args {
                    mcp_prefix: Some("bad.*".to_string()),
                    client_id_prefix: None,
                    server_id: None,
                    host: None,
                    port: None,
                    allowed_hosts: Vec::new(),
                }),
                &env,
            ),
            Err(ConfigError::Prefix(_))
        ));
        assert!(matches!(
            base_config(
                &FixedArgs(Args {
                    mcp_prefix: None,
                    client_id_prefix: Some("bad.client".to_string()),
                    server_id: None,
                    host: None,
                    port: None,
                    allowed_hosts: Vec::new(),
                }),
                &env,
            ),
            Err(ConfigError::ClientIdPrefix(_))
        ));
        assert!(matches!(
            base_config(
                &FixedArgs(Args {
                    mcp_prefix: None,
                    client_id_prefix: None,
                    server_id: Some("bad.server".to_string()),
                    host: None,
                    port: None,
                    allowed_hosts: Vec::new(),
                }),
                &env,
            ),
            Err(ConfigError::ServerId(_))
        ));
        env.set("MCP_HTTP_HOST", "bad-host");
        assert!(matches!(
            base_config(&args(), &env),
            Err(ConfigError::InvalidHost { .. })
        ));
        env.remove("MCP_HTTP_HOST");
        env.set("MCP_HTTP_PORT", "bad-port");
        assert!(matches!(
            base_config(&args(), &env),
            Err(ConfigError::InvalidPort { .. })
        ));
        env.remove("MCP_HTTP_PORT");
        assert!(matches!(
            base_config(
                &FixedArgs(Args {
                    mcp_prefix: None,
                    client_id_prefix: None,
                    server_id: None,
                    host: None,
                    port: None,
                    allowed_hosts: vec!["bad host".to_string()],
                }),
                &env,
            ),
            Err(ConfigError::AllowedHost(_))
        ));
    }

    #[test]
    fn config_error_display_and_source_are_specific() {
        let prefix = err(base_config(
            &FixedArgs(Args {
                mcp_prefix: Some("bad.*".to_string()),
                client_id_prefix: None,
                server_id: None,
                host: None,
                port: None,
                allowed_hosts: Vec::new(),
            }),
            &InMemoryEnv::new(),
        ));
        assert_eq!(prefix.to_string(), "invalid MCP prefix");
        assert!(std::error::Error::source(&prefix).is_some());

        let client_id_prefix = err(base_config(
            &FixedArgs(Args {
                mcp_prefix: None,
                client_id_prefix: Some("bad.client".to_string()),
                server_id: None,
                host: None,
                port: None,
                allowed_hosts: Vec::new(),
            }),
            &InMemoryEnv::new(),
        ));
        assert_eq!(client_id_prefix.to_string(), "invalid MCP client id prefix");
        assert!(std::error::Error::source(&client_id_prefix).is_some());

        let server_id = err(base_config(
            &FixedArgs(Args {
                mcp_prefix: None,
                client_id_prefix: None,
                server_id: Some("bad.server".to_string()),
                host: None,
                port: None,
                allowed_hosts: Vec::new(),
            }),
            &InMemoryEnv::new(),
        ));
        assert_eq!(server_id.to_string(), "invalid MCP server id");
        assert!(std::error::Error::source(&server_id).is_some());

        let allowed_host = err(base_config(
            &FixedArgs(Args {
                mcp_prefix: None,
                client_id_prefix: None,
                server_id: None,
                host: None,
                port: None,
                allowed_hosts: vec!["bad host".to_string()],
            }),
            &InMemoryEnv::new(),
        ));
        assert_eq!(allowed_host.to_string(), "invalid MCP HTTP allowed host");
        assert!(std::error::Error::source(&allowed_host).is_some());

        let env = InMemoryEnv::new();
        env.set("MCP_HTTP_HOST", "bad-host");
        let invalid_host = err(base_config(&args(), &env));
        assert_eq!(
            invalid_host.to_string(),
            "invalid value for MCP_HTTP_HOST: \"bad-host\" (invalid IP address syntax)"
        );
        assert!(std::error::Error::source(&invalid_host).is_some());

        let env = InMemoryEnv::new();
        env.set("MCP_HTTP_PORT", "bad-port");
        let invalid_port = err(base_config(&args(), &env));

        assert_eq!(
            invalid_port.to_string(),
            "invalid value for MCP_HTTP_PORT: \"bad-port\" (invalid digit found in string)"
        );
        assert!(std::error::Error::source(&invalid_port).is_some());
    }

    #[test]
    #[should_panic(expected = "expected invalid MCP HTTP bridge config")]
    fn err_panics_when_config_is_valid() {
        let _ = err(base_config(&args(), &InMemoryEnv::new()));
    }
