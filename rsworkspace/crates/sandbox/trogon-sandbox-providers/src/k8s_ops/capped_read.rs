use tokio::io::{AsyncRead, AsyncReadExt};
use trogon_sandbox_contract::OutputTruncation;
use trogon_sandbox_protocol::CappedOutput;

use crate::k8s_ops::constants::EXEC_READ_CHUNK_BYTES;

/// Reads `reader` to its end, retaining at most `cap` bytes (head and tail)
/// so a runaway command cannot grow the worker's memory without bound.
/// Returns the retained bytes and whether anything was dropped.
pub(crate) async fn read_capped<R>(mut reader: R, cap: usize) -> std::io::Result<(Vec<u8>, OutputTruncation)>
where
    R: AsyncRead + Unpin,
{
    let mut capped = CappedOutput::new(cap);
    let mut chunk = [0u8; EXEC_READ_CHUNK_BYTES];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        capped.push(&chunk[..n]);
    }
    let truncated = capped.truncation();
    Ok((capped.into_bytes(), truncated))
}

/// Reads `reader` to its end keeping only the last `tail` bytes.
pub(crate) async fn read_tail<R>(mut reader: R, tail: usize) -> std::io::Result<(Vec<u8>, OutputTruncation)>
where
    R: AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut truncation = OutputTruncation::Complete;
    let mut chunk = [0u8; EXEC_READ_CHUNK_BYTES];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > tail {
            let excess = buf.len() - tail;
            buf.drain(..excess);
            truncation = OutputTruncation::Truncated;
        }
    }
    Ok((buf, truncation))
}
