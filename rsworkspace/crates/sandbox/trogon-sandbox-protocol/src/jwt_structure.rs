use base64::Engine as _;
use base64::engine::general_purpose::{URL_SAFE_NO_PAD, URL_SAFE_NO_PAD_INDIFFERENT};
use serde_json::{Map, Value};

use crate::JwtDefect;

/// Signature segment of every fake token.
pub(crate) const FAKE_SIGNATURE: &str = "ZmFrZS1zaWduYXR1cmU";

pub(crate) struct ParsedJwt<'a> {
    pub(crate) header: &'a str,
    pub(crate) payload: Map<String, Value>,
}

fn decode_object(segment: &str) -> Result<Option<Map<String, Value>>, ()> {
    let bytes = URL_SAFE_NO_PAD_INDIFFERENT.decode(segment).map_err(|_| ())?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(map)) => Ok(Some(map)),
        Ok(_) => Ok(None),
        Err(_) => Err(()),
    }
}

fn is_url_safe(segment: &str) -> bool {
    segment
        .trim_end_matches('=')
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub(crate) fn parse(token: &str) -> Result<ParsedJwt<'_>, JwtDefect> {
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(JwtDefect::SegmentCount);
    };
    let well_formed =
        !header.is_empty() && !payload.is_empty() && [header, payload, signature].into_iter().all(is_url_safe);
    if !well_formed {
        return Err(JwtDefect::Encoding);
    }
    let header_object = decode_object(header)
        .map_err(|()| JwtDefect::HeaderNotJson)?
        .ok_or(JwtDefect::HeaderNotJson)?;
    if !header_object.get("alg").is_some_and(Value::is_string) {
        return Err(JwtDefect::HeaderWithoutAlg);
    }
    let payload = decode_object(payload)
        .map_err(|()| JwtDefect::PayloadNotJson)?
        .ok_or(JwtDefect::PayloadNotObject)?;
    Ok(ParsedJwt { header, payload })
}

/// True when `candidate` is structurally a JWT: three base64url segments, a JSON object header declaring a string
/// `alg`, and a JSON object payload.
pub fn is_jwt(candidate: &str) -> bool {
    parse(candidate).is_ok()
}

pub(crate) fn is_fake_jwt(raw: &str) -> bool {
    is_jwt(raw) && raw.rsplit('.').next() == Some(FAKE_SIGNATURE)
}

pub(crate) fn encode(value: &Value) -> String {
    URL_SAFE_NO_PAD.encode(value.to_string())
}

fn is_segment_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

fn skip_run(bytes: &[u8], from: usize) -> usize {
    bytes.get(from..).map_or(from, |rest| {
        from + rest.iter().take_while(|byte| is_segment_byte(**byte)).count()
    })
}

fn skip_padding(bytes: &[u8], from: usize) -> usize {
    from + bytes
        .get(from..)
        .map_or(0, |rest| rest.iter().take(2).take_while(|byte| **byte == b'=').count())
}

fn expect_dot(bytes: &[u8], at: usize) -> Option<usize> {
    (bytes.get(at) == Some(&b'.')).then_some(at + 1)
}

/// Byte ranges of the `eyJ...`-led three-segment candidates in `content`, leftmost first.
pub(crate) fn candidate_ranges(content: &str) -> Vec<std::ops::Range<usize>> {
    let bytes = content.as_bytes();
    let mut ranges = Vec::new();
    let mut start = 0;
    while let Some(offset) = content.get(start..).and_then(|rest| rest.find("eyJ")) {
        let begin = start + offset;
        let first_end = skip_run(bytes, begin + 3);
        let matched = (first_end > begin + 3)
            .then(|| {
                let after_first = skip_padding(bytes, first_end);
                let second_start = expect_dot(bytes, after_first)?;
                let second_end = skip_run(bytes, second_start);
                if second_end == second_start {
                    return None;
                }
                let third_start = expect_dot(bytes, skip_padding(bytes, second_end))?;
                Some(skip_padding(bytes, skip_run(bytes, third_start)))
            })
            .flatten();
        match matched {
            Some(end) => {
                ranges.push(begin..end);
                start = end;
            }
            None => start = first_end.max(begin + 1),
        }
    }
    ranges
}
