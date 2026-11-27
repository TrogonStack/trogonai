use trogon_sandbox_contract::{CommandOutput, CommandTimeoutStatus, OutputRetention, OutputStream, OutputTruncation};
use trogon_sandbox_protocol::{CappedOutput, effective_max_output_bytes};

/// Accumulates what a command printed according to how the caller asked for it to be kept.
pub(crate) struct OutputCollector {
    retention: OutputRetention,
    stdout: CappedOutput,
    stderr: CappedOutput,
    stderr_tail: Vec<u8>,
}

impl OutputCollector {
    pub(crate) fn new(retention: OutputRetention, max_output_bytes: Option<u64>) -> Self {
        let cap = effective_max_output_bytes(max_output_bytes, usize::MAX);
        Self {
            retention,
            stdout: CappedOutput::new(cap),
            stderr: CappedOutput::new(cap),
            stderr_tail: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, stream: OutputStream, data: &[u8]) {
        match (stream, self.retention) {
            (OutputStream::Stdout, OutputRetention::Buffer) => self.stdout.push(data),
            // Stream-only output was already delivered as chunks, so only a bounded tail of stderr is kept for the
            // final answer's diagnostics.
            (OutputStream::Stdout, OutputRetention::StreamOnly) => {}
            (OutputStream::Stderr, OutputRetention::Buffer) => self.stderr.push(data),
            (OutputStream::Stderr, OutputRetention::StreamOnly) => self.keep_tail(data),
        }
    }

    fn keep_tail(&mut self, data: &[u8]) {
        self.stderr_tail.extend_from_slice(data);
        let excess = self
            .stderr_tail
            .len()
            .saturating_sub(OutputRetention::STREAM_ONLY_STDERR_TAIL_BYTES);
        self.stderr_tail.drain(..excess);
    }

    pub(crate) fn finish(self, exit_code: i32, timeout: Option<u32>) -> CommandOutput {
        let truncated =
            OutputTruncation::from(self.stdout.truncation().is_truncated() || self.stderr.truncation().is_truncated());
        let mut stderr = match self.retention {
            OutputRetention::StreamOnly => self.stderr_tail,
            OutputRetention::Buffer => self.stderr.into_bytes(),
        };
        let timed_out = CommandTimeoutStatus::from(timeout.is_some());
        let exit_code = timeout.map_or(exit_code, |seconds| {
            stderr.extend_from_slice(format!("command timed out after {seconds}s").as_bytes());
            CommandOutput::TIMED_OUT_EXIT_CODE
        });
        CommandOutput {
            exit_code,
            stdout: self.stdout.into_bytes(),
            stderr,
            timed_out,
            truncated,
        }
    }
}

#[cfg(test)]
mod tests;
