#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignatureVerificationError {
    #[error("skill {skill_id}: missing signature file at {path}")]
    MissingSignatureFile { skill_id: String, path: String },
    #[error("skill {skill_id}: malformed signature envelope: {detail}")]
    MalformedSignatureFile { skill_id: String, detail: String },
    #[error("skill {skill_id}: manifest sha256 mismatch")]
    ManifestSha256Mismatch { skill_id: String },
    #[error("skill {skill_id}: wasm sha256 mismatch")]
    WasmSha256Mismatch { skill_id: String },
    #[error("skill {skill_id}: ed25519 signature verification failed")]
    SignatureVerificationFailed { skill_id: String },
}
