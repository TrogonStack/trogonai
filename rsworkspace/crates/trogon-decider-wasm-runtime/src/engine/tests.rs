use super::*;

#[test]
fn default_config_uses_documented_defaults() {
    let config = WasmEngineConfig::default();
    assert_eq!(config.max_memory_bytes(), DEFAULT_MAX_MEMORY_BYTES);
    assert_eq!(config.fuel_per_call(), DEFAULT_FUEL_PER_CALL);
}

#[test]
fn builder_methods_override_defaults() {
    let config = WasmEngineConfig::default()
        .with_max_memory_bytes(4096)
        .with_fuel_per_call(1);
    assert_eq!(config.max_memory_bytes(), 4096);
    assert_eq!(config.fuel_per_call(), 1);
}

#[test]
fn new_builds_a_component_model_engine() {
    let engine = WasmDeciderEngine::new(WasmEngineConfig::default());
    assert!(engine.is_ok());
}
