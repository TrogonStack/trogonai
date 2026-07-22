#[derive(Debug, thiserror::Error)]
pub enum RedactionError {
    #[error("wasm engine initialization failed: {0}")]
    WasmEngine(String),
    #[error("wasm module compile failed: {0}")]
    WasmModule(String),
    #[error("wasm module instantiation failed: {0}")]
    WasmInstance(String),
    #[error("wasm redaction abi mismatch: {0}")]
    WasmAbi(String),
    #[error("wasm redact_part call failed: {0}")]
    WasmCall(String),
    #[error("wasm linear memory access failed: {0}")]
    WasmMemory(String),
    /// JSON encode/decode of a `Part` payload failed. Preserves the typed
    /// `serde_json::Error` as `source()` so error chains survive past this
    /// boundary instead of being flattened to a String.
    #[error("json serialization for redaction failed: {0}")]
    Json(#[from] serde_json::Error),
    /// The Tier-3 wasm guest emitted the `A2A_T3_REFUSE` sentinel; the
    /// optional payload after the colon is the reason tag (e.g.
    /// `UnauthorizedDataCategory`). Surfacing this as a typed variant lets
    /// callers route refusals separately from generic JSON / wasm failures.
    #[error("tier-3 skill refused redaction{}", .0.as_ref().map(|tag| format!(": {tag}")).unwrap_or_default())]
    Tier3Refusal(Option<String>),
    /// Signed-bundle verification failed loading a skill (missing sig,
    /// malformed envelope, digest mismatch, ed25519 verify failure). The
    /// typed `SignatureVerificationError` preserves the *kind* of failure
    /// so callers can route on it instead of pattern-matching error text.
    #[error("signed bundle verification failed: {0}")]
    Signature(#[from] crate::signed_bundle::SignatureVerificationError),
}

#[cfg(test)]
mod tests;
