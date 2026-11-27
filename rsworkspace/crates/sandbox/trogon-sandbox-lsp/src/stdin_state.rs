use std::collections::VecDeque;

use trogon_sandbox_contract::{StdinChunk, StdinEof, StdinSeq};

use crate::lsp_error::LspError;

#[derive(Debug, Clone)]
pub(crate) struct PendingChunk {
    pub(crate) seq: StdinSeq,
    pub(crate) data: StdinChunk,
    pub(crate) eof: StdinEof,
}

#[derive(Debug, Default)]
pub(crate) struct StdinState {
    next_seq: u64,
    pub(crate) eof_queued: bool,
    pub(crate) failure: Option<LspError>,
    pub(crate) unacked: VecDeque<PendingChunk>,
}

impl StdinState {
    /// Splits `data` into numbered chunks, putting `eof` on the last one.
    pub(crate) fn queue_chunks(&mut self, data: &[u8], eof: StdinEof) -> Result<(), LspError> {
        if data.is_empty() && !eof.closes() {
            return Ok(());
        }
        let mut pieces: Vec<&[u8]> = data.chunks(StdinChunk::MAX_LEN).collect();
        if pieces.is_empty() {
            pieces.push(&[]);
        }
        let last = pieces.len() - 1;
        for (index, piece) in pieces.into_iter().enumerate() {
            self.unacked.push_back(PendingChunk {
                seq: StdinSeq::new(self.next_seq),
                data: StdinChunk::new(piece.to_vec())?,
                eof: StdinEof::from(eof.closes() && index == last),
            });
            self.next_seq += 1;
        }
        if eof.closes() {
            self.eof_queued = true;
        }
        Ok(())
    }
}
