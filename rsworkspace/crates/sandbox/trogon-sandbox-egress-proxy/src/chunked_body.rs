use crate::head_parse_error::HeadParseError;
use crate::http_intercept_error::HttpInterceptError;
use crate::refusal_error::RefusalError;

pub(crate) fn decode_chunked_body(body: &[u8]) -> Result<Vec<u8>, RefusalError> {
    let malformed = || RefusalError::MalformedRequest("malformed chunked body");
    let mut out = Vec::with_capacity(body.len());
    let mut pos = 0;
    loop {
        let line_end = body
            .get(pos..)
            .ok_or_else(malformed)?
            .windows(2)
            .position(|w| w == b"\r\n")
            .map(|offset| pos + offset)
            .ok_or_else(malformed)?;
        let size_text = body[pos..line_end].split(|b| *b == b';').next().unwrap_or_default();
        let size = std::str::from_utf8(size_text)
            .ok()
            .and_then(|text| usize::from_str_radix(text, 16).ok())
            .ok_or_else(malformed)?;
        pos = line_end + 2;
        if size == 0 {
            return Ok(out);
        }
        let chunk_end = pos.checked_add(size).ok_or_else(malformed)?;
        out.extend_from_slice(body.get(pos..chunk_end).ok_or_else(malformed)?);
        pos = chunk_end.checked_add(2).ok_or_else(malformed)?;
    }
}

pub(crate) fn parse_chunk_size(line: &[u8]) -> Result<usize, HttpInterceptError> {
    let size_text = line.split(|b| *b == b';').next().unwrap_or_default();
    let Some(size_text) = std::str::from_utf8(size_text)
        .ok()
        .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
    else {
        return Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(
            HeadParseError::InvalidChunkSize,
        )));
    };
    usize::from_str_radix(size_text, 16).map_err(|_| HttpInterceptError::Refused(RefusalError::BodyTooLarge))
}
