use trogon_sandbox_contract::{CommandOutputChunk, OutputStream};

use crate::lsp_error::LspError;
use crate::output_chunk::OutputChunk;
use crate::output_kind::OutputKind;

pub(crate) fn decode_chunk(payload: &[u8]) -> Result<OutputChunk, LspError> {
    let chunk: CommandOutputChunk = serde_json::from_slice(payload)
        .map_err(|e| LspError::Protocol(format!("invalid command output frame: {e}")))?;
    Ok(OutputChunk {
        kind: match chunk.stream {
            OutputStream::Stdout => OutputKind::Stdout,
            OutputStream::Stderr => OutputKind::Stderr,
        },
        seq: chunk.seq,
        data: chunk.data,
    })
}
