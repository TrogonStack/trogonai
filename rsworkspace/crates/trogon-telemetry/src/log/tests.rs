use super::*;
    use opentelemetry::KeyValue;
    use trogon_std::env::InMemoryEnv;
    use trogon_std::fs::MemFs;

    #[test]
    fn ensure_log_dir_uses_env_override() {
        let env = InMemoryEnv::new();
        env.set("TROGON_LOG_DIR", "/custom/logs");
        let fs = MemFs::new();

        let dir = ensure_log_dir(ServiceName::AcpNatsStdio, &env, &fs).unwrap();
        assert_eq!(dir, PathBuf::from("/custom/logs"));
        assert!(fs.dir_exists(&PathBuf::from("/custom/logs")));
    }

    #[test]
    fn ensure_log_dir_ignores_legacy_acp_env_override() {
        let env = InMemoryEnv::new();
        env.set("ACP_LOG_DIR", "/legacy/logs");
        let fs = MemFs::new();

        let dir = ensure_log_dir(ServiceName::AcpNatsStdio, &env, &fs).unwrap();

        assert!(dir.ends_with(ServiceName::AcpNatsStdio.as_str()));
        assert!(!fs.dir_exists(&PathBuf::from("/legacy/logs")));
    }

    #[test]
    fn ensure_log_dir_falls_back_to_platform() {
        let env = InMemoryEnv::new();
        let fs = MemFs::new();

        let dir = ensure_log_dir(ServiceName::AcpNatsStdio, &env, &fs).unwrap();
        assert!(dir.ends_with(ServiceName::AcpNatsStdio.as_str()));
    }

    #[test]
    fn platform_log_dir_returns_path_ending_with_service_name() {
        let dir = platform_log_dir(ServiceName::AcpNatsServer).unwrap();
        assert!(dir.ends_with(ServiceName::AcpNatsServer.as_str()));
    }

    #[test]
    fn init_provider_lifecycle() {
        let resource = opentelemetry_sdk::Resource::builder()
            .with_service_name("test-log")
            .with_attributes(vec![KeyValue::new("test", "true")])
            .build();

        let provider = init_provider(&resource);
        assert!(provider.is_ok());
    }
