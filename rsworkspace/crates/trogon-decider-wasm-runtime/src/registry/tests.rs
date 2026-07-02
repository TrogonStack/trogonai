use super::*;
use crate::test_fixture::schedules_bytes;
use crate::{WasmDeciderEngine, WasmEngineConfig};

fn schedules_module() -> WasmDeciderModule {
    let engine = WasmDeciderEngine::new(WasmEngineConfig::default()).expect("engine builds");
    WasmDeciderModule::load(engine, &schedules_bytes()).expect("module loads")
}

#[test]
fn routes_registered_command_types() {
    let registry = DeciderRegistry::builder()
        .register(schedules_module())
        .expect("registration succeeds")
        .build();

    let command_type =
        CommandType::new(trogonai_proto::scheduler::schedules::CREATE_SCHEDULE_TYPE_URL).expect("valid command type");
    let module = registry.route(&command_type).expect("route succeeds");
    assert_eq!(module.name().as_str(), "scheduler.schedules");
}

#[test]
fn unknown_command_type_is_reported() {
    let registry = DeciderRegistry::builder()
        .register(schedules_module())
        .expect("registration succeeds")
        .build();

    let command_type = CommandType::new("NoSuchCommand").expect("valid command type");
    let Err(error) = registry.route(&command_type) else {
        panic!("expected unknown command type error");
    };
    assert_eq!(error.command_type, command_type);
}

#[test]
fn duplicate_command_type_across_modules_is_rejected() {
    let builder = DeciderRegistry::builder()
        .register(schedules_module())
        .expect("first registration succeeds");

    let Err(error) = builder.register(schedules_module()) else {
        panic!("expected duplicate command type error");
    };
    assert!(matches!(error, RegisterModuleError::DuplicateCommandType { .. }));
}
