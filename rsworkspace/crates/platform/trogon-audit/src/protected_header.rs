use serde::Deserialize;

use crate::checkpoint_jws;

/// A checkpoint JWS's protected header as a verifier reads it. A header with any
/// other member, `crit` and `b64` included, is refused, because the writer writes
/// only these three and a member it never sets could change what was signed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProtectedHeader {
    pub(crate) alg: String,
    pub(crate) kid: String,
    pub(crate) typ: String,
}

impl ProtectedHeader {
    pub(crate) fn decode(segment: &str) -> Option<Self> {
        let json = checkpoint_jws::decode_segment(segment)?;
        serde_json::from_slice(&json).ok()
    }
}

#[cfg(test)]
mod tests;
