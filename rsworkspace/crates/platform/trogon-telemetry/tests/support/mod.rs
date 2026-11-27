//! Shared harness for tests that exercise process-wide telemetry state in a child process.

use std::process::{Command, Output};

use trogon_std::env::{InMemoryEnv, ReadEnv, SystemEnv};
use trogon_std::fs::MemFs;
use trogon_telemetry::{ResourceAttribute, ServiceName};

/// Re-runs the calling test in a child process and returns its output, or `None` when
/// called from inside that child.
///
/// libtest names the test thread after the test, so the child runs exactly this one.
/// `--nocapture` lets the child's `eprintln!` warnings reach its stderr. `configure` adds
/// the environment the scenario needs.
pub fn rerun_as_child(marker: &str, configure: impl FnOnce(&mut Command) -> &mut Command) -> Option<Output> {
    if SystemEnv.var(marker).is_ok() {
        return None;
    }

    let test_name = std::thread::current().name().unwrap().to_owned();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", &test_name, "--test-threads=1", "--nocapture"])
        .env(marker, "1");
    Some(configure(&mut command).output().unwrap())
}

/// Runs `init_logger` the way every child scenario does, logging to files under `log_dir`.
pub fn init_test_logger(log_dir: &str, fs: &MemFs) {
    let env = InMemoryEnv::new();
    env.set("TROGON_LOG_DIR", log_dir);
    trogon_telemetry::init_logger(
        ServiceName::AcpNatsStdio,
        [ResourceAttribute::acp_prefix("test")],
        &env,
        fs,
    );
}
