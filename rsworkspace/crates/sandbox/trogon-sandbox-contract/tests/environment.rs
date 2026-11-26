use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{EnvDigest, EnvironmentDefinition, LegacySnapshotPolicy, ProviderKind};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn definition_starts_at_the_current_schema_version() {
    let definition = EnvironmentDefinition::new(ProviderKind::Firecracker, "x86_64");

    assert_eq!(EnvironmentDefinition::SCHEMA_VERSION, 1);
    assert_eq!(definition.schema_version, EnvironmentDefinition::SCHEMA_VERSION);
    assert_eq!(definition.architecture, "x86_64");
    assert!(definition.components.is_empty());
    assert_eq!(definition.cpu_template, None);
}

#[test]
fn definition_keeps_the_wire_field_order_and_sorts_components() -> TestResult {
    let definition = EnvironmentDefinition::new(ProviderKind::Firecracker, "x86_64")
        .with_component("kernel", "v6")
        .with_component("vmm_version", "v1.9.0")
        .with_component("kernel", "v7");

    assert_eq!(
        serde_json::to_string(&definition)?,
        r#"{"schema_version":1,"provider":"firecracker","architecture":"x86_64","components":{"kernel":"v7","vmm_version":"v1.9.0"},"boot_args":"","snapshot_format":0,"isolation_profile":"","cpu_template":null}"#
    );
    let decoded: EnvironmentDefinition = serde_json::from_str(&serde_json::to_string(&definition)?)?;
    assert_eq!(decoded, definition);
    Ok(())
}

#[test]
fn definition_rejects_an_unknown_provider() -> TestResult {
    let error = serde_json::from_value::<EnvironmentDefinition>(json!({
        "schema_version": 1,
        "provider": "nope",
        "architecture": "x86_64",
        "components": {},
        "boot_args": "",
        "snapshot_format": 0,
        "isolation_profile": "",
        "cpu_template": null
    }))
    .err()
    .ok_or("expected error")?;

    assert_eq!(error.classify(), Data);
    Ok(())
}

#[test]
fn env_digest_names_its_annotation() {
    assert_eq!(EnvDigest::ANNOTATION_KEY, "io.trogon.sandbox.env-digest");
}

#[test]
fn legacy_snapshot_policy_defaults_to_allow_and_uses_snake_case() -> TestResult {
    assert_eq!(LegacySnapshotPolicy::default(), LegacySnapshotPolicy::AllowWithWarning);
    assert_eq!(
        serde_json::to_string(&LegacySnapshotPolicy::AllowWithWarning)?,
        "\"allow_with_warning\""
    );
    assert_eq!(
        serde_json::from_str::<LegacySnapshotPolicy>("\"refuse\"")?,
        LegacySnapshotPolicy::Refuse
    );
    let error = serde_json::from_str::<LegacySnapshotPolicy>("\"Refuse\"")
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}
