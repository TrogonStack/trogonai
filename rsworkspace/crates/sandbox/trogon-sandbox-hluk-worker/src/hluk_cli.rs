use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use trogon_sandbox_contract::{CommandOutput, CommandTimeoutStatus};

use crate::constants::KILLED_EXIT_CODE;
use crate::{GuestDir, GuestNetwork, HlukCliError, HlukLease, HlukRuntime};

/// The `hluk` binary, driven one process per command.
#[derive(Debug, Clone)]
pub struct HlukCli {
    bin: PathBuf,
}

impl HlukCli {
    pub fn new(bin: PathBuf) -> Self {
        Self { bin }
    }

    /// Pulls `runtime`'s image and saves its warm snapshot, so a lease's commands restore instead of boot.
    pub async fn warm(&self, runtime: HlukRuntime) -> Result<(), HlukCliError> {
        let output = self
            .command(runtime)
            .args(["--exec", runtime.noop()])
            .stdin(Stdio::null())
            .output()
            .await?;
        if output.status.success() {
            return Ok(());
        }
        Err(HlukCliError::Warm {
            runtime: runtime.image_name(),
            code: output.status.code().unwrap_or(KILLED_EXIT_CODE),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Runs `code` in a guest restored from `lease`'s runtime snapshot with its workspace and `/tmp` mounted, killing
    /// it at `timeout`.
    pub async fn run(
        &self,
        lease: &HlukLease,
        env: &BTreeMap<String, String>,
        code: &str,
        timeout: Duration,
    ) -> Result<CommandOutput, HlukCliError> {
        let mut command = self.command(lease.runtime);
        for dir in GuestDir::ALL {
            let read_only = if lease.read_only(dir) { ":ro" } else { "" };
            command.arg("--mount").arg(format!(
                "{}:{}{read_only}",
                lease.host_dir(dir).display(),
                dir.guest_path()
            ));
        }
        for (key, value) in env {
            command.arg("--env").arg(format!("{key}={value}"));
        }
        if lease.network == GuestNetwork::Open {
            command.arg("--net");
        }
        command
            .arg("--exec")
            .arg(code)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = command.spawn()?;
        // Dropping the child at the deadline kills it.
        let Ok(output) = tokio::time::timeout(timeout, child.wait_with_output()).await else {
            return Ok(CommandOutput {
                exit_code: CommandOutput::TIMED_OUT_EXIT_CODE,
                timed_out: CommandTimeoutStatus::TimedOut,
                ..Default::default()
            });
        };
        let output = output?;
        Ok(CommandOutput {
            exit_code: output.status.code().unwrap_or(KILLED_EXIT_CODE),
            stdout: output.stdout,
            stderr: output.stderr,
            ..Default::default()
        })
    }

    fn command(&self, runtime: HlukRuntime) -> Command {
        let mut command = Command::new(&self.bin);
        command.args(["run", "--runtime", runtime.image_name()]);
        command
    }
}
