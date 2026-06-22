use std::fmt;

#[derive(Debug)]
pub enum RedactionError {
    WasmEngine(String),
    WasmModule(String),
    WasmInstance(String),
    WasmAbi(String),
    WasmCall(String),
    WasmMemory(String),
    Json(String),
}

impl fmt::Display for RedactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WasmEngine(msg) => write!(f, "wasm engine initialization failed: {msg}"),
            Self::WasmModule(msg) => write!(f, "wasm module compile failed: {msg}"),
            Self::WasmInstance(msg) => write!(f, "wasm module instantiation failed: {msg}"),
            Self::WasmAbi(msg) => write!(f, "wasm redaction abi mismatch: {msg}"),
            Self::WasmCall(msg) => write!(f, "wasm redact_part call failed: {msg}"),
            Self::WasmMemory(msg) => write!(f, "wasm linear memory access failed: {msg}"),
            Self::Json(msg) => write!(f, "json serialization for redaction failed: {msg}"),
        }
    }
}

impl std::error::Error for RedactionError {}

impl From<serde_json::Error> for RedactionError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasm_engine_variant_carries_underlying_message() {
        let inner = "module init failed";
        let wrapped = RedactionError::WasmEngine(inner.to_string());
        assert!(
            wrapped.to_string().contains("wasm engine initialization failed"),
            "{wrapped}"
        );
        assert!(wrapped.to_string().contains(inner));
    }
}
