//! `redact_part(in_ptr,in_len):(out_ptr,out_len)` transports UTF-8 JSON through guest-exported linear `memory`.

use wasmtime::{Engine, Instance, Linker, Module, Store};

use crate::error::RedactionError;

const SCRATCH_OFFSET: usize = 0x0800;
const GUEST_PAGE_BYTES: usize = 65536;

pub(crate) fn new_engine() -> Result<Engine, RedactionError> {
    let mut config = wasmtime::Config::default();
    config.wasm_multi_value(true);
    Engine::new(&config).map_err(|e| RedactionError::WasmEngine(e.to_string()))
}

pub(crate) fn redact_part_guest(
    engine: &Engine,
    module: &Module,
    payload: &[u8],
) -> Result<Vec<u8>, RedactionError> {
    if SCRATCH_OFFSET.saturating_add(payload.len()) > GUEST_PAGE_BYTES {
        return Err(RedactionError::WasmMemory(
            "redaction payload does not fit in one wasm guest page".into(),
        ));
    }

    let in_len = i32::try_from(payload.len()).map_err(|_| {
        RedactionError::WasmMemory("payload length does not fit in wasm i32 bounds".into())
    })?;

    let mut store = Store::new(engine, ());
    let linker: Linker<()> = Linker::new(engine);
    let instance: Instance = linker
        .instantiate(&mut store, module)
        .map_err(|e| RedactionError::WasmInstance(e.to_string()))?;

    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| RedactionError::WasmAbi("wasm module must export linear memory named memory".into()))?;

    memory
        .write(&mut store, SCRATCH_OFFSET, payload)
        .map_err(|e| RedactionError::WasmMemory(e.to_string()))?;

    let redact = instance
        .get_typed_func::<(i32, i32), (i32, i32)>(&mut store, "redact_part")
        .map_err(|_| {
            RedactionError::WasmAbi("wasm module must export redact_part with type (i32,i32)->(i32,i32)".into())
        })?;

    let (out_base, out_len) = redact
        .call(&mut store, (SCRATCH_OFFSET as i32, in_len))
        .map_err(|e| RedactionError::WasmCall(e.to_string()))?;

    let out_base = usize::try_from(out_base).map_err(|_| {
        RedactionError::WasmAbi("wasm redact_part returned negative output pointer".into())
    })?;
    let out_len = usize::try_from(out_len)
        .map_err(|_| RedactionError::WasmAbi("wasm redact_part returned negative output length".into()))?;

    let mut dst = vec![0u8; out_len];
    memory
        .read(&store, out_base, &mut dst)
        .map_err(|e| RedactionError::WasmMemory(e.to_string()))?;

    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::Module;

    #[test]
    fn builds_default_compatible_engine() {
        let engine = new_engine().expect("wasmtime engine");
        drop(engine);
    }

    #[test]
    fn identity_guest_round_trips_utf8_payload() {
        let engine = new_engine().unwrap();
        let wasm = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/identity_redact_part.wasm"
        ));
        let module = Module::from_binary(&engine, wasm).unwrap();
        let inp = br#"{"k":42}"#.to_vec();
        let got = redact_part_guest(&engine, &module, &inp).unwrap();
        assert_eq!(got, inp);
    }
}
