use trogon_sandbox_contract::OutputTruncation;

/// Output of a pod exec captured via `K8sOps::exec_with_output`: real
/// stdout/stderr bytes plus the process exit code (0 when the kube exec API
/// reports `Status: Success` with no `ExitCode` cause, i.e. the common case).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PodExecOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// stdout or stderr exceeded the retention cap and its middle was dropped.
    pub truncated: OutputTruncation,
    /// The command outlived its deadline and was signaled; `exit_code` is -1.
    pub timed_out: bool,
}

impl PodExecOutput {
    /// The result reported for a command that outlived `timeout_seconds`.
    pub fn timed_out_after(timeout_seconds: u64) -> Self {
        Self {
            exit_code: -1,
            stderr: format!("command timed out after {timeout_seconds}s").into_bytes(),
            timed_out: true,
            ..Default::default()
        }
    }
}
