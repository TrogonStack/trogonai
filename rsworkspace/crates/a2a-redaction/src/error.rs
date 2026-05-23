use std::fmt;

#[derive(Debug)]
pub enum RedactionError {
    WasmEngine(String),
}

impl fmt::Display for RedactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WasmEngine(msg) => write!(f, "wasm engine initialization failed: {msg}"),
        }
    }
}

impl std::error::Error for RedactionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::{Engine, Module};

    #[test]
    fn wasm_engine_includes_underlying_diagnostic() {
        let inner = Module::from_binary(&Engine::default(), &[0u8]).unwrap_err();
        let inner_msg = inner.to_string();
        let wrapped = RedactionError::WasmEngine(inner_msg.clone());
        assert!(
            wrapped.to_string().contains("wasm engine initialization failed"),
            "{}",
            wrapped
        );
        assert!(wrapped.to_string().contains(&inner_msg));
    }
}
