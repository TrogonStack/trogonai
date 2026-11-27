use reqwest::Response;

use crate::collected::Collected;
use crate::constants::LINE_LIMIT;
use crate::{VercelApiError, VercelExecError};

/// Feeds the NDJSON body of `response` into `collected` as it arrives, so a caller that times out keeps the partial
/// output.
pub(crate) async fn drain(response: Response, collected: &mut Collected) -> Result<(), VercelExecError> {
    drain_within(response, collected, LINE_LIMIT).await
}

async fn drain_within(
    mut response: Response,
    collected: &mut Collected,
    line_limit: usize,
) -> Result<(), VercelExecError> {
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(VercelApiError::transport)? {
        if buf.len().saturating_add(chunk.len()) > line_limit {
            return Err(VercelExecError::Capture("native capture JSON record exceeds its bound"));
        }
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            collected.line(&line[..pos]);
        }
    }
    if !buf.is_empty() {
        collected.line(&buf);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
