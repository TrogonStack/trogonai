use crate::admission::Admission;
use crate::lsp_error::LspError;
use crate::output_chunk::OutputChunk;
use crate::output_kind::OutputKind;

/// Tracks the per-stream chunk counters. A repeated chunk is dropped; a skipped stdout chunk
/// would corrupt the framing, so it is an error. stderr is best-effort text and only deduplicated.
#[derive(Debug, Default)]
pub struct OutputSequencer {
    last_stdout: Option<u64>,
    last_stderr: Option<u64>,
}

impl OutputSequencer {
    pub fn admit(&mut self, chunk: &OutputChunk) -> Result<Admission, LspError> {
        match chunk.kind {
            OutputKind::Stderr => {
                if self.last_stderr.is_some_and(|last| chunk.seq <= last) {
                    return Ok(Admission::Skip);
                }
                self.last_stderr = Some(chunk.seq);
                Ok(Admission::Accept)
            }
            OutputKind::Stdout => {
                if self.last_stdout.is_some_and(|last| chunk.seq <= last) {
                    return Ok(Admission::Skip);
                }
                let expected = self.last_stdout.map_or(0, |last| last + 1);
                if chunk.seq != expected {
                    return Err(LspError::Protocol(format!(
                        "missing stdout seq {expected} (received seq {})",
                        chunk.seq
                    )));
                }
                self.last_stdout = Some(chunk.seq);
                Ok(Admission::Accept)
            }
        }
    }
}
