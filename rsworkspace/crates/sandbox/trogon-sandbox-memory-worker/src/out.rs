use trogon_sandbox_contract::CommandOutput;

/// A finished command's output from text.
pub fn out(exit_code: i32, stdout: &str, stderr: &str) -> CommandOutput {
    CommandOutput {
        exit_code,
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
        ..Default::default()
    }
}
