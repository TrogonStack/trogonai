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
}

#[cfg(test)]
mod tests;
