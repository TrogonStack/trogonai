use serde::Serialize;

use crate::lsp_error::LspError;

/// Frames `message` as JSON behind a `Content-Length` header.
pub fn encode_message<T: Serialize + ?Sized>(message: &T) -> Result<Vec<u8>, LspError> {
    let body = serde_json::to_vec(message)?;
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    Ok(out)
}
