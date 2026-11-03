//! Reads one HTTP/1.1 request head and decides whether it is a CONNECT
//! (ADR#0085 Decision 2).
//!
//! Only the request line and the header *names* matter. No header value is
//! kept or logged. The head is bounded, strictly CRLF framed and ASCII, and a
//! request that announces a body is refused: after the blank line the bytes
//! belong to the tunnel, so a body would make the framing ambiguous.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::constants::{HEAD_READ_CHUNK, HEAD_TERMINATOR};
use crate::destination::{Destination, DestinationError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Connect(Destination),
    /// A well-formed request with any other method.
    OtherMethod,
}

/// Which part of the head is malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedPart {
    NonAsciiOrControlByte,
    BareLineEnding,
    RequestLine,
    Method,
    Version,
    HeaderLine,
    HeaderName,
}

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("request head is larger than {max} bytes")]
    HeadTooLarge { max: usize },
    #[error("connection closed before the request head was complete")]
    Closed,
    #[error("request head is malformed: {0:?}")]
    Malformed(MalformedPart),
    #[error("request announces a body, which CONNECT does not carry")]
    AnnouncesBody,
    #[error("CONNECT target is not a valid host:port")]
    InvalidTarget(#[source] DestinationError),
    #[error("reading the request failed")]
    Io(#[source] io::Error),
}

/// Reads until the blank line that ends the head, never holding more than
/// `max_head` bytes of head. Returns the parsed request and any bytes the
/// client sent after the head, which belong to the tunnel.
pub async fn read_request<R>(reader: &mut R, max_head: usize) -> Result<(Request, Vec<u8>), RequestError>
where
    R: AsyncRead + Unpin,
{
    let mut buffer: Vec<u8> = Vec::with_capacity(HEAD_READ_CHUNK);
    let mut scanned = 0;
    loop {
        if let Some(position) = find(&buffer[scanned..], HEAD_TERMINATOR) {
            let head_end = scanned + position + HEAD_TERMINATOR.len();
            if head_end > max_head {
                return Err(RequestError::HeadTooLarge { max: max_head });
            }
            let request = parse_head(&buffer[..head_end])?;
            return Ok((request, buffer.split_off(head_end)));
        }
        if buffer.len() >= max_head {
            return Err(RequestError::HeadTooLarge { max: max_head });
        }
        scanned = buffer.len().saturating_sub(HEAD_TERMINATOR.len() - 1);
        let room = HEAD_READ_CHUNK.min(max_head - buffer.len());
        let mut chunk = [0u8; HEAD_READ_CHUNK];
        let read = reader.read(&mut chunk[..room]).await.map_err(RequestError::Io)?;
        if read == 0 {
            return Err(RequestError::Closed);
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// Parses a complete head, including its terminating blank line.
pub fn parse_head(head: &[u8]) -> Result<Request, RequestError> {
    if head
        .iter()
        .any(|b| !(b.is_ascii() && (!b.is_ascii_control() || matches!(b, b'\r' | b'\n' | b'\t'))))
    {
        return Err(RequestError::Malformed(MalformedPart::NonAsciiOrControlByte));
    }
    let text = std::str::from_utf8(head).map_err(|_| RequestError::Malformed(MalformedPart::NonAsciiOrControlByte))?;
    let block = text
        .strip_suffix("\r\n\r\n")
        .ok_or(RequestError::Malformed(MalformedPart::BareLineEnding))?;
    let mut lines = block.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    if lines
        .clone()
        .chain([request_line])
        .any(|line| line.contains(['\r', '\n']))
    {
        return Err(RequestError::Malformed(MalformedPart::BareLineEnding));
    }

    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(RequestError::Malformed(MalformedPart::RequestLine));
    };
    if method.is_empty() || !method.bytes().all(is_token_byte) {
        return Err(RequestError::Malformed(MalformedPart::Method));
    }
    if target.is_empty() {
        return Err(RequestError::Malformed(MalformedPart::RequestLine));
    }
    if !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return Err(RequestError::Malformed(MalformedPart::Version));
    }

    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or(RequestError::Malformed(MalformedPart::HeaderLine))?;
        if name.is_empty() || !name.bytes().all(is_token_byte) {
            return Err(RequestError::Malformed(MalformedPart::HeaderName));
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(RequestError::AnnouncesBody);
        }
        if name.eq_ignore_ascii_case("content-length") && value.trim_matches([' ', '\t']) != "0" {
            return Err(RequestError::AnnouncesBody);
        }
    }

    if method == "CONNECT" {
        target
            .parse::<Destination>()
            .map(Request::Connect)
            .map_err(RequestError::InvalidTarget)
    } else {
        Ok(Request::OtherMethod)
    }
}

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
        )
}

#[cfg(test)]
mod tests;
