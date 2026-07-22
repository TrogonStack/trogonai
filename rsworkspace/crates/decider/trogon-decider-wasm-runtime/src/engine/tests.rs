use wasmtime::{Instance, Memory, MemoryType, Module, Ref, RefType, Table, TableType};

use super::*;

#[test]
fn default_config_uses_documented_defaults() {
    let config = WasmEngineConfig::default();
    assert_eq!(config.max_memory_bytes(), DEFAULT_MAX_MEMORY_BYTES);
    assert_eq!(config.fuel_per_call(), DEFAULT_FUEL_PER_CALL);
    assert_eq!(config.max_table_elements(), DEFAULT_MAX_TABLE_ELEMENTS);
    assert_eq!(config.max_instances_per_session(), DEFAULT_MAX_INSTANCES_PER_SESSION);
    assert_eq!(config.max_tables_per_session(), DEFAULT_MAX_TABLES_PER_SESSION);
    assert_eq!(config.max_memories_per_session(), DEFAULT_MAX_MEMORIES_PER_SESSION);
    assert_eq!(config.max_concurrent_sessions(), DEFAULT_MAX_CONCURRENT_SESSIONS);
    assert_eq!(config.epoch_tick_interval(), DEFAULT_EPOCH_TICK_INTERVAL);
    assert_eq!(config.epoch_ticks_per_call(), DEFAULT_EPOCH_TICKS_PER_CALL);
}

#[test]
fn builder_methods_override_defaults() {
    let config = WasmEngineConfig::default()
        .with_max_memory_bytes(4096)
        .with_fuel_per_call(1)
        .with_max_table_elements(16)
        .with_max_instances_per_session(2)
        .with_max_tables_per_session(3)
        .with_max_memories_per_session(4)
        .with_max_concurrent_sessions(5)
        .with_epoch_tick_interval(Duration::from_millis(7))
        .with_epoch_ticks_per_call(9);
    assert_eq!(config.max_memory_bytes(), 4096);
    assert_eq!(config.fuel_per_call(), 1);
    assert_eq!(config.max_table_elements(), 16);
    assert_eq!(config.max_instances_per_session(), 2);
    assert_eq!(config.max_tables_per_session(), 3);
    assert_eq!(config.max_memories_per_session(), 4);
    assert_eq!(config.max_concurrent_sessions(), 5);
    assert_eq!(config.epoch_tick_interval(), Duration::from_millis(7));
    assert_eq!(config.epoch_ticks_per_call(), 9);
}

#[test]
fn new_builds_a_component_model_engine() {
    let engine = WasmDeciderEngine::new(WasmEngineConfig::default());
    assert!(engine.is_ok());
}

#[test]
fn engine_error_preserves_the_wasmtime_configure_source() {
    let error = WasmEngineError::Configure(wasmtime::Error::msg("boom"));
    assert_eq!(error.to_string(), "failed to configure wasmtime engine");
    let source = std::error::Error::source(&error).expect("source is preserved");
    assert_eq!(source.to_string(), "boom");
}

#[test]
fn engine_error_preserves_the_epoch_ticker_source() {
    let error = WasmEngineError::EpochTicker(std::io::Error::other("boom"));
    assert_eq!(
        error.to_string(),
        "failed to start the epoch interruption ticker thread"
    );
    let source = std::error::Error::source(&error).expect("source is preserved");
    assert_eq!(source.to_string(), "boom");
}

#[test]
fn memory_growth_beyond_the_configured_ceiling_is_rejected() {
    let engine =
        WasmDeciderEngine::new(WasmEngineConfig::default().with_max_memory_bytes(64 * 1024)).expect("engine builds");
    let mut store = engine.new_store();
    let memory = Memory::new(&mut store, MemoryType::new(1, None)).expect("memory at the ceiling is allowed");
    assert!(memory.grow(&mut store, 1).is_err());
}

#[test]
fn table_growth_beyond_the_configured_ceiling_is_rejected() {
    let engine = WasmDeciderEngine::new(WasmEngineConfig::default().with_max_table_elements(4)).expect("engine builds");
    let mut store = engine.new_store();
    let table = Table::new(&mut store, TableType::new(RefType::FUNCREF, 4, None), Ref::Func(None))
        .expect("table at the ceiling is allowed");
    assert!(table.grow(&mut store, 1, Ref::Func(None)).is_err());
}

/// The minimal valid core wasm module: just the `\0asm` magic and version
/// header, no sections. Used instead of the WAT text format because this
/// crate disables wasmtime's default features and does not pull in the `wat`
/// text-to-binary parser.
const EMPTY_MODULE_BYTES: [u8; 8] = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

#[test]
fn instance_count_beyond_the_configured_ceiling_is_rejected() {
    let engine =
        WasmDeciderEngine::new(WasmEngineConfig::default().with_max_instances_per_session(1)).expect("engine builds");
    let module = Module::new(engine.engine(), EMPTY_MODULE_BYTES).expect("trivial module compiles");
    let mut store = engine.new_store();
    Instance::new(&mut store, &module, &[]).expect("first instance is allowed");
    assert!(Instance::new(&mut store, &module, &[]).is_err());
}

#[test]
fn arm_guest_call_sets_fuel_and_a_non_zero_epoch_deadline() {
    let engine = WasmDeciderEngine::new(WasmEngineConfig::default()).expect("engine builds");
    let mut store = engine.new_store();
    assert!(engine.arm_guest_call(&mut store, 100, 10).is_ok());
}

/// The minimal core wasm module with one exported function that does nothing.
const EMPTY_EXPORTED_FUNCTION_MODULE_BYTES: [u8; 31] = [
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00, 0x03, 0x02, 0x01, 0x00, 0x07,
    0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x04, 0x01, 0x02, 0x00, 0x0b,
];

#[test]
fn a_zero_tick_epoch_deadline_traps_a_guest_call() {
    let engine = WasmDeciderEngine::new(WasmEngineConfig::default()).expect("engine builds");
    let module = Module::new(engine.engine(), EMPTY_EXPORTED_FUNCTION_MODULE_BYTES).expect("trivial module compiles");
    let mut store = engine.new_store();
    let instance = Instance::new(&mut store, &module, &[]).expect("instance");
    let func = instance
        .get_typed_func::<(), ()>(&mut store, "f")
        .expect("exported function f");

    engine
        .arm_guest_call(&mut store, 1_000_000, 0)
        .expect("arming a zero-tick deadline succeeds");
    let error = func
        .call(&mut store, ())
        .expect_err("a zero-tick deadline is already elapsed");

    assert!(
        matches!(error.downcast_ref::<wasmtime::Trap>(), Some(wasmtime::Trap::Interrupt)),
        "{error}"
    );
}
