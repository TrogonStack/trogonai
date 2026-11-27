use tokio::io::{AsyncRead, AsyncReadExt};

use super::connect_response_error::ConnectResponseError;

/// Reads a CONNECT response's status line and headers one byte at a time, stopping at the
/// first blank line.
///
/// A buffered read here could pull bytes belonging to the tunneled HTTP/1.1 traffic off the
/// wire along with the CONNECT response and discard them, so this reads no further than the
/// blank line that ends the response headers.
pub(super) async fn read_connect_response<S: AsyncRead + Unpin + ?Sized>(
    stream: &mut S,
) -> Result<(), ConnectResponseError> {
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        let n = stream.read(&mut byte).await.map_err(ConnectResponseError::Read)?;
        if n == 0 {
            return Err(ConnectResponseError::Closed);
        }
        buffer.push(byte[0]);
    }

    let status_line = buffer.split(|b| *b == b'\n').next().unwrap_or_default();
    let status_line = String::from_utf8_lossy(status_line);
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| ConnectResponseError::UnreadableStatusLine {
            line: status_line.to_string(),
        })?;
    if !(200..300).contains(&status) {
        return Err(ConnectResponseError::Refused {
            status,
            line: status_line.trim().to_string(),
        });
    }
    Ok(())
}
