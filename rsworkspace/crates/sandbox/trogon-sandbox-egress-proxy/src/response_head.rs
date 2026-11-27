use tokio::io::{AsyncRead, AsyncReadExt};

use crate::constants::MAX_HEAD_BYTES;
use crate::http_intercept_error::HttpInterceptError;

pub(crate) fn has_opaque_content_encoding(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-encoding"))
        .flat_map(|(_, value)| value.split(','))
        .any(|coding| {
            let coding = coding.trim();
            !coding.is_empty() && !coding.eq_ignore_ascii_case("identity")
        })
}

pub(crate) async fn read_response_header_end<S>(stream: &mut S, buf: &mut Vec<u8>) -> Result<usize, HttpInterceptError>
where
    S: AsyncRead + Unpin,
{
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(pos) = find_header_terminator(buf) {
            return Ok(pos);
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(HttpInterceptError::MalformedRequest(
                "response headers too large".into(),
            ));
        }
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(HttpInterceptError::UpstreamRead)?;
        if n == 0 {
            return Err(HttpInterceptError::UpstreamRead(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "upstream closed mid-response",
            )));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

pub(crate) fn parse_response_headers(header_text: &str) -> Vec<(String, String)> {
    header_text
        .split("\r\n")
        .skip(1)
        .filter(|line| !line.is_empty())
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect()
}

pub(crate) fn find_header_terminator(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|pos| pos + 4)
}

#[cfg(test)]
mod tests;
