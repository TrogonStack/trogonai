use std::fmt;

/// Why a value is not a verifiable JWT.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum JwtDefect {
    SegmentCount,
    Encoding,
    HeaderNotJson,
    HeaderWithoutAlg,
    PayloadNotJson,
    PayloadNotObject,
}

impl fmt::Display for JwtDefect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SegmentCount => "it does not have exactly three dot-separated segments",
            Self::Encoding => "a segment is not base64url",
            Self::HeaderNotJson => "the header is not a JSON object",
            Self::HeaderWithoutAlg => "the header does not declare an alg",
            Self::PayloadNotJson => "the payload is not JSON",
            Self::PayloadNotObject => "the payload is not a JSON object",
        })
    }
}
