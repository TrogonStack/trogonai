use super::*;

#[test]
fn renders_module_and_stream_identity() {
    let module_name = ModuleName::new("scheduler.schedules").unwrap();
    let module_version = ModuleVersion::new("0.1.0").unwrap();
    let snapshot_id = WasmSnapshotId::new(&module_name, &module_version, "alpha");

    assert_eq!(snapshot_id.as_str(), "scheduler.schedules@0.1.0/alpha");
    assert_eq!(snapshot_id.to_string(), "scheduler.schedules@0.1.0/alpha");
    assert_eq!(snapshot_id.as_ref(), "scheduler.schedules@0.1.0/alpha");
}

#[test]
fn version_bump_changes_identity() {
    let module_name = ModuleName::new("scheduler.schedules").unwrap();
    let previous = WasmSnapshotId::new(&module_name, &ModuleVersion::new("0.1.0").unwrap(), "alpha");
    let next = WasmSnapshotId::new(&module_name, &ModuleVersion::new("0.2.0").unwrap(), "alpha");

    assert_ne!(previous, next);
}
