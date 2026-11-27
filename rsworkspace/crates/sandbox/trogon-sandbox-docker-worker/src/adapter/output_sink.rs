use super::*;

/// Where one exec's already-redacted output goes: published as a streamed
/// chunk when streaming, and kept in the capped buffers `CommandOutput` is
/// built from.
pub(super) struct OutputSink<'a> {
    pub(super) streaming: &'a Option<StreamingContext>,
    pub(super) retention: OutputRetention,
    pub(super) stdout: &'a mut CappedOutput,
    pub(super) stderr: &'a mut CappedOutput,
    pub(super) stderr_tail: &'a mut Vec<u8>,
}

impl OutputSink<'_> {
    /// Takes the next safe bytes of `stream`. Empty input (a redactor holding
    /// everything back) produces no chunk, so streamed sequence numbers stay
    /// gapless.
    pub(super) async fn accept(
        &mut self,
        sequencer: &mut ChunkSequencer,
        stream: trogon_sandbox_contract::OutputStream,
        data: Vec<u8>,
    ) {
        if data.is_empty() {
            return;
        }
        if let Some(ctx) = self.streaming {
            DockerWorkerProvider::publish_output_chunk(ctx, sequencer, stream, data.clone()).await;
        }
        match stream {
            trogon_sandbox_contract::OutputStream::Stdout => {
                if self.retention == OutputRetention::Buffer {
                    self.stdout.push(&data);
                }
            }
            trogon_sandbox_contract::OutputStream::Stderr => {
                if self.retention == OutputRetention::StreamOnly {
                    self.stderr_tail.extend_from_slice(&data);
                    if self.stderr_tail.len() > STREAM_ONLY_STDERR_TAIL {
                        let excess = self.stderr_tail.len() - STREAM_ONLY_STDERR_TAIL;
                        self.stderr_tail.drain(..excess);
                    }
                } else {
                    self.stderr.push(&data);
                }
            }
        }
    }
}
