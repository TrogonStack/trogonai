use super::*;
use crate::WasmEngineConfig;
use crate::test_fixture::schedules_bytes;

fn test_engine() -> WasmDeciderEngine {
    WasmDeciderEngine::new(WasmEngineConfig::default()).expect("engine builds")
}

#[test]
fn loads_a_zero_import_component_and_probes_its_descriptor() {
    let module = WasmDeciderModule::load(test_engine(), &schedules_bytes()).expect("module loads");

    assert_eq!(module.name().as_str(), "scheduler.schedules");
    assert_eq!(module.version().as_str(), "0.1.0");
    let command_types: Vec<&str> = module.command_types().collect();
    assert!(command_types.contains(&trogonai_proto::scheduler::schedules::CREATE_SCHEDULE_TYPE_URL));
    assert!(command_types.contains(&trogonai_proto::scheduler::schedules::PAUSE_SCHEDULE_TYPE_URL));
}

#[test]
fn rejects_a_component_that_is_not_a_valid_wasm_component() {
    let error = WasmDeciderModule::load(test_engine(), b"not a wasm component");
    assert!(matches!(error, Err(LoadWasmDeciderError::Compile { .. })));
}

#[test]
fn duplicate_command_type_error_names_the_offending_type() {
    let descriptor = ModuleDescriptor {
        name: "module".to_string(),
        version: "1.0.0".to_string(),
        commands: vec![
            trogon_decider_wit::host::CommandSpec {
                command_type: "Foo".to_string(),
                write_precondition: None,
            },
            trogon_decider_wit::host::CommandSpec {
                command_type: "Foo".to_string(),
                write_precondition: None,
            },
        ],
    };

    let error = ensure_unique_command_types(&descriptor).unwrap_err();
    assert_eq!(
        error,
        InvalidDescriptorError::DuplicateCommandType {
            command_type: "Foo".to_string()
        }
    );
}

#[test]
fn unique_command_types_pass_validation() {
    let descriptor = ModuleDescriptor {
        name: "module".to_string(),
        version: "1.0.0".to_string(),
        commands: vec![trogon_decider_wit::host::CommandSpec {
            command_type: "Foo".to_string(),
            write_precondition: None,
        }],
    };

    assert!(ensure_unique_command_types(&descriptor).is_ok());
}
