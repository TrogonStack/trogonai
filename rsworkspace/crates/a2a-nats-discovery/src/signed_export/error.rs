use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignedExportError {
    UnknownKeyId(String),
    SignatureMismatch,
    PayloadDigestMismatch,
    StaleSignature { signed_at_unix_ms: u64, now_unix_ms: u64, max_age_ms: u64 },
    Malformed(String),
}

impl fmt::Display for SignedExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKeyId(key_id) => write!(f, "unknown operator key id `{key_id}`"),
            Self::SignatureMismatch => write!(f, "operator export signature mismatch"),
            Self::PayloadDigestMismatch => write!(f, "operator export payload digest mismatch"),
            Self::StaleSignature {
                signed_at_unix_ms,
                now_unix_ms,
                max_age_ms,
            } => write!(
                f,
                "operator export signature stale (signed_at={signed_at_unix_ms}, now={now_unix_ms}, max_age_ms={max_age_ms})"
            ),
            Self::Malformed(detail) => write!(f, "malformed operator export envelope: {detail}"),
        }
    }
}

impl std::error::Error for SignedExportError {}
