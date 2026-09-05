use std::process::Command;
use std::sync::{Arc, Mutex};

use log::{Log, Metadata, Record};

use crate::env::{ReadEnv, SystemEnv};

pub use log::Level as LogLevel;

mod constants;

#[derive(Clone)]
pub struct CapturedLog {
    pub level: LogLevel,
    pub target: String,
    pub message: String,
}

/// Tests the log facade before any tracing subscriber has been installed.
/// Tracing remembers subscriber installation for the process lifetime, so a
/// fresh test process prevents unrelated tests from disabling its log fallback.
#[derive(Clone, Default)]
pub struct CapturedLogs(Arc<Mutex<Vec<CapturedLog>>>);

impl CapturedLogs {
    /// Returns a capture in the child, or `None` after the parent verifies that
    /// the same test completed successfully in its isolated process.
    pub fn isolated() -> Option<Self> {
        let thread = std::thread::current();
        let name = thread.name().expect("libtest names its test threads");
        if SystemEnv.var(constants::CHILD_TEST).is_ok_and(|child| child == name) {
            let logs = Self::default();
            log::set_boxed_logger(Box::new(logs.clone())).expect("the isolated test owns its logger");
            log::set_max_level(log::LevelFilter::Trace);
            return Some(logs);
        }

        let output = Command::new(std::env::current_exe().expect("the test executable is available"))
            .args(["--exact", name, "--nocapture", "--test-threads=1"])
            .env(constants::CHILD_TEST, name)
            .output()
            .expect("the isolated test process starts");
        assert!(
            output.status.success(),
            "isolated log test failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        None
    }

    pub fn records(&self) -> Vec<CapturedLog> {
        self.flush();
        self.0.lock().expect("captured logs are not poisoned").clone()
    }
}

impl Log for CapturedLogs {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &Record<'_>) {
        self.0
            .lock()
            .expect("captured logs are not poisoned")
            .push(CapturedLog {
                level: record.level(),
                target: record.target().to_owned(),
                message: record.args().to_string(),
            });
    }

    fn flush(&self) {}
}

#[cfg(test)]
mod tests;
