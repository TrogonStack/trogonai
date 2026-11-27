use crate::constants::{HEADER_TERMINATOR, MAX_BODY_BYTES, MAX_HEADER_BYTES};
use crate::lsp_error::LspError;

/// Byte-based `Content-Length` framing for LSP/JSON-RPC streams. Lengths count
/// bytes, so bodies are only decoded once complete and multi-byte UTF-8 split
/// across chunks is safe.
#[derive(Debug, Default)]
pub struct LspFramer {
    buffer: Vec<u8>,
}

impl LspFramer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the raw JSON body of every message completed by this chunk.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, LspError> {
        self.buffer.extend_from_slice(chunk);
        let mut bodies = Vec::new();
        let mut consumed = 0;
        loop {
            let rest = self.buffer.get(consumed..).unwrap_or_default();
            let Some(header_end) = find_terminator(rest) else {
                if rest.len() > MAX_HEADER_BYTES {
                    return Err(LspError::Protocol("LSP header section exceeds limit".into()));
                }
                break;
            };
            let length = parse_content_length(decode(rest.get(..header_end).unwrap_or_default())?)?;
            let body_start = header_end + HEADER_TERMINATOR.len();
            let Some(body) = rest.get(body_start..body_start + length) else {
                break;
            };
            bodies.push(decode(body)?.to_owned());
            consumed += body_start + length;
        }
        self.buffer.drain(..consumed);
        Ok(bodies)
    }
}

fn decode(bytes: &[u8]) -> Result<&str, LspError> {
    std::str::from_utf8(bytes).map_err(|_| LspError::Protocol("LSP message is not valid UTF-8".into()))
}

fn find_terminator(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(HEADER_TERMINATOR.len())
        .position(|window| window == HEADER_TERMINATOR)
}

fn parse_content_length(header: &str) -> Result<usize, LspError> {
    for line in header.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("content-length") {
            continue;
        }
        let value = value.trim();
        let invalid = || LspError::Protocol(format!("invalid Content-Length: {value}"));
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        let length: usize = value.parse().map_err(|_| invalid())?;
        if length > MAX_BODY_BYTES {
            return Err(LspError::Protocol(format!("Content-Length {length} exceeds limit")));
        }
        return Ok(length);
    }
    Err(LspError::Protocol("LSP message is missing Content-Length".into()))
}
