use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureVerificationError {
    MissingSignatureFile {
        skill_id: String,
        path: String,
    },
    MalformedSignatureFile {
        skill_id: String,
        detail: String,
    },
    ManifestSha256Mismatch {
        skill_id: String,
    },
    WasmSha256Mismatch {
        skill_id: String,
    },
    SignatureVerificationFailed {
        skill_id: String,
    },
}

impl fmt::Display for SignatureVerificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSignatureFile { skill_id, path } => {
                write!(f, "skill {skill_id}: missing signature file at {path}")
            }
            Self::MalformedSignatureFile { skill_id, detail } => {
                write!(f, "skill {skill_id}: malformed signature envelope: {detail}")
            }
            Self::ManifestSha256Mismatch { skill_id } => {
                write!(f, "skill {skill_id}: manifest sha256 mismatch")
            }
            Self::WasmSha256Mismatch { skill_id } => {
                write!(f, "skill {skill_id}: wasm sha256 mismatch")
            }
            Self::SignatureVerificationFailed { skill_id } => {
                write!(f, "skill {skill_id}: ed25519 signature verification failed")
            }
        }
    }
}

impl std::error::Error for SignatureVerificationError {}
