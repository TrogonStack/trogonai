use acp_telemetry::ServiceName;
use trogon_std::env::InMemoryEnv;
use trogon_std::fs::MemFs;

#[test]
fn init_logger_creates_log_dir_and_shuts_down_cleanly() {
    let env = InMemoryEnv::new();
    env.set("ACP_LOG_DIR", "/tmp/test-coverage-lifecycle");
    let fs = MemFs::new();

    acp_telemetry::init_logger(ServiceName::AcpNatsStdio, "test", &env, &fs);

    assert!(
        fs.dir_exists(&std::path::PathBuf::from("/tmp/test-coverage-lifecycle")),
        "init_logger should create the configured log directory"
    );

    assert!(acp_telemetry::shutdown_otel().is_ok());
}
