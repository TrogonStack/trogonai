use trogon_std::env::InMemoryEnv;
use trogon_std::fs::MemFs;
use trogon_telemetry::{ResourceAttribute, ServiceName};

#[test]
fn init_logger_creates_log_dir_and_shuts_down_cleanly() {
    let env = InMemoryEnv::new();
    env.set("TROGON_LOG_DIR", "/tmp/test-coverage-lifecycle");
    let fs = MemFs::new();

    trogon_telemetry::init_logger(
        ServiceName::AcpNatsStdio,
        [ResourceAttribute::acp_prefix("test")],
        &env,
        &fs,
    );

    assert!(
        fs.dir_exists(&std::path::PathBuf::from("/tmp/test-coverage-lifecycle")),
        "init_logger should create the configured log directory"
    );

    assert!(trogon_telemetry::shutdown_otel().is_ok());
}
