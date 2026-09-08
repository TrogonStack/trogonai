use trogon_decider_runtime::{SnapshotCadence, WritePrecondition};
use trogon_decider_wit::host;

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
    let command_types: Vec<&str> = module.command_types().map(CommandType::as_str).collect();
    assert!(command_types.contains(&trogonai_proto::scheduler::schedules::CREATE_SCHEDULE_TYPE_URL));
    assert!(command_types.contains(&trogonai_proto::scheduler::schedules::PAUSE_SCHEDULE_TYPE_URL));
    let create_spec = module
        .commands()
        .iter()
        .find(|spec| spec.command_type().as_str() == trogonai_proto::scheduler::schedules::CREATE_SCHEDULE_TYPE_URL)
        .expect("create command is declared");
    assert_eq!(create_spec.write_precondition(), WritePrecondition::NoStream);
}

#[test]
fn rejects_a_component_that_is_not_a_valid_wasm_component() {
    let error = WasmDeciderModule::load(test_engine(), b"not a wasm component");
    assert!(matches!(error, Err(LoadWasmDeciderError::Compile { .. })));
}

#[test]
fn rejects_a_component_that_declares_an_import() {
    let bytes = wat::parse_str(
        r#"
        (component
          (import "trogon:decider/handler" (instance
            (export "descriptor" (func (result string)))
          ))
        )
        "#,
    )
    .expect("component with one import parses");

    let error = WasmDeciderModule::load(test_engine(), &bytes);
    assert!(matches!(error, Err(LoadWasmDeciderError::ForbiddenImports { .. })));
}

#[test]
fn duplicate_command_type_error_names_the_offending_type() {
    let descriptor = ModuleDescriptor {
        name: "module".to_string(),
        version: "1.0.0".to_string(),
        commands: vec![
            host::CommandSpec {
                command_type: "Foo".to_string(),
                write_precondition: host::WritePrecondition::StreamUnchanged,
                snapshot_policy: host::SnapshotPolicy::NoSnapshot,
            },
            host::CommandSpec {
                command_type: "Foo".to_string(),
                write_precondition: host::WritePrecondition::StreamUnchanged,
                snapshot_policy: host::SnapshotPolicy::NoSnapshot,
            },
        ],
    };

    let error = validate_commands(descriptor).unwrap_err();
    assert_eq!(
        error,
        InvalidDescriptorError::DuplicateCommandType {
            command_type: CommandType::new("Foo").expect("valid command type")
        }
    );
}

#[test]
fn unique_command_types_carry_their_declared_policies_through_validation() {
    let descriptor = ModuleDescriptor {
        name: "module".to_string(),
        version: "1.0.0".to_string(),
        commands: vec![host::CommandSpec {
            command_type: "Foo".to_string(),
            write_precondition: host::WritePrecondition::StreamExists,
            snapshot_policy: host::SnapshotPolicy::Frequency(64),
        }],
    };

    let commands = validate_commands(descriptor).expect("unique command types validate");
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].command_type().as_str(), "Foo");
    assert_eq!(commands[0].write_precondition(), WritePrecondition::StreamExists);
    assert_eq!(commands[0].snapshot_cadence(), SnapshotCadence::every_events(64));
}

#[test]
fn an_invalid_command_type_preserves_the_validation_source() {
    let descriptor = ModuleDescriptor {
        name: "module".to_string(),
        version: "1.0.0".to_string(),
        commands: vec![host::CommandSpec {
            command_type: String::new(),
            write_precondition: host::WritePrecondition::StreamUnchanged,
            snapshot_policy: host::SnapshotPolicy::NoSnapshot,
        }],
    };

    let error = validate_commands(descriptor).unwrap_err();
    assert!(matches!(error, InvalidDescriptorError::InvalidCommandType(_)));
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn invalid_name_and_version_preserve_the_validation_source() {
    let Err(name_error) = crate::ModuleName::new("") else {
        panic!("empty module name must be invalid");
    };
    let error = InvalidDescriptorError::InvalidName(name_error);
    assert_eq!(
        error.to_string(),
        "wasm component descriptor has an invalid module name"
    );
    assert!(std::error::Error::source(&error).is_some());

    let Err(version_error) = crate::ModuleVersion::new("") else {
        panic!("empty module version must be invalid");
    };
    let error = InvalidDescriptorError::InvalidVersion(version_error);
    assert_eq!(
        error.to_string(),
        "wasm component descriptor has an invalid module version"
    );
    assert!(std::error::Error::source(&error).is_some());
}
