use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use tokio::process::Command;
use tracing::debug;

use crate::GuestExecError;
use crate::constants::{DEFAULT_KUBECTL, DEFAULT_VIRTCTL};

/// Runs commands inside a KubeVirt guest. It prefers `virtctl guestos exec` and falls back to `kubectl exec` into the
/// VM's virt-launcher pod when `virtctl` is unavailable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestExec {
    /// The `virtctl` program.
    pub virtctl: PathBuf,
    /// The `kubectl` program.
    pub kubectl: PathBuf,
}

impl Default for GuestExec {
    fn default() -> Self {
        Self {
            virtctl: PathBuf::from(DEFAULT_VIRTCTL),
            kubectl: PathBuf::from(DEFAULT_KUBECTL),
        }
    }
}

impl GuestExec {
    /// Runs `command` through `bash -lc` in VM `vm_name` of `namespace` and returns its standard output.
    pub async fn run(&self, namespace: &str, vm_name: &str, command: &str) -> Result<String, GuestExecError> {
        if self.virtctl_available().await {
            debug!(namespace, vm_name, "exec via virtctl guestos");
            let args = [
                "guestos",
                "exec",
                vm_name,
                "--namespace",
                namespace,
                "--",
                "bash",
                "-lc",
                command,
            ];
            let output = output(&self.virtctl, &args).await?;
            return stdout(output, |stderr| GuestExecError::Virtctl { stderr });
        }
        let launcher = self.launcher_pod(namespace, vm_name).await?;
        debug!(namespace, pod = %launcher, "exec via kubectl into virt-launcher");
        let args = [
            "exec", "-n", namespace, &launcher, "-c", "compute", "--", "bash", "-lc", command,
        ];
        let output = output(&self.kubectl, &args).await?;
        stdout(output, |stderr| GuestExecError::Kubectl { stderr })
    }

    /// Adds `public_key` (an `authorized_keys` line) for the guest's `sandbox` user, once.
    pub async fn append_authorized_key(
        &self,
        namespace: &str,
        vm_name: &str,
        public_key: &str,
    ) -> Result<(), GuestExecError> {
        let escaped = shell_escape(public_key);
        let command = format!(
            "mkdir -p /home/sandbox/.ssh && chmod 700 /home/sandbox/.ssh && \
             grep -qxF '{escaped}' /home/sandbox/.ssh/authorized_keys 2>/dev/null || \
             echo '{escaped}' >> /home/sandbox/.ssh/authorized_keys && \
             chown -R sandbox:sandbox /home/sandbox/.ssh && chmod 600 /home/sandbox/.ssh/authorized_keys"
        );
        self.run(namespace, vm_name, &command).await?;
        Ok(())
    }

    /// Removes the lines equal to `public_key` from the guest's `sandbox` user. A guest without an
    /// `authorized_keys` file is left alone.
    pub async fn remove_authorized_key(
        &self,
        namespace: &str,
        vm_name: &str,
        public_key: &str,
    ) -> Result<(), GuestExecError> {
        let escaped = shell_escape(public_key);
        let command = format!(
            "f=/home/sandbox/.ssh/authorized_keys; \
             if [ -f \"$f\" ]; then \
             grep -vxF '{escaped}' \"$f\" > \"$f.new\"; rc=$?; \
             if [ \"$rc\" -gt 1 ]; then rm -f \"$f.new\"; exit \"$rc\"; fi; \
             cat \"$f.new\" > \"$f\" && rm -f \"$f.new\"; \
             fi"
        );
        self.run(namespace, vm_name, &command).await?;
        Ok(())
    }

    async fn virtctl_available(&self) -> bool {
        let mut command = Command::new(&self.virtctl);
        command.arg("version").stdout(Stdio::null()).stderr(Stdio::null());
        command.status().await.is_ok_and(|status| status.success())
    }

    async fn launcher_pod(&self, namespace: &str, vm_name: &str) -> Result<String, GuestExecError> {
        let selector = format!("kubevirt.io=virt-launcher,kubevirt.io/domain={vm_name}");
        let args = [
            "get",
            "pods",
            "-n",
            namespace,
            "-l",
            &selector,
            "-o",
            "jsonpath={.items[0].metadata.name}",
        ];
        let output = output(&self.kubectl, &args).await?;
        let stdout = stdout(output, |stderr| GuestExecError::LauncherLookup {
            vm_name: vm_name.to_string(),
            stderr,
        })?;
        let name = stdout.trim();
        if name.is_empty() {
            return Err(GuestExecError::LauncherNotFound {
                vm_name: vm_name.to_string(),
            });
        }
        Ok(name.to_string())
    }
}

/// Quotes `value` for use inside a single-quoted shell word.
fn shell_escape(value: &str) -> String {
    value.replace('\'', "'\\''")
}

async fn output(program: &Path, args: &[&str]) -> Result<Output, GuestExecError> {
    let result = Command::new(program).args(args).output().await;
    result.map_err(|source| GuestExecError::Spawn {
        program: program.to_path_buf(),
        source,
    })
}

/// The standard output of a successful run, or `failed` with the standard error of an unsuccessful one.
fn stdout(output: Output, failed: impl FnOnce(String) -> GuestExecError) -> Result<String, GuestExecError> {
    if !output.status.success() {
        return Err(failed(String::from_utf8_lossy(&output.stderr).into_owned()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
