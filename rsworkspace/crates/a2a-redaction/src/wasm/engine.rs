use wasmtime::Engine;

use crate::error::RedactionError;

pub(crate) fn new_engine() -> Result<Engine, RedactionError> {
    let config = wasmtime::Config::default();
    Engine::new(&config).map_err(|e| RedactionError::WasmEngine(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_default_compatible_engine() {
        let engine = new_engine().expect("default wasmtime engine config");
        let _keep = engine;
    }
}
