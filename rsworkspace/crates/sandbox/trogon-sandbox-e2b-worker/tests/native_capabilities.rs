use trogon_sandbox_e2b_worker::PROFILE;

#[test]
fn native_memory_snapshots_admit_capture_and_restore_without_claiming_oci_export() {
    assert!(PROFILE.snapshot.supports_native_snapshots.is_supported());
    assert!(PROFILE.snapshot.can_checkpoint_running.is_supported());
    assert!(PROFILE.snapshot.can_restore.is_supported());
    assert!(!PROFILE.snapshot.can_snapshot.is_supported());
    assert!(PROFILE.snapshot.supported_artifact_types.is_empty());
    assert!(!PROFILE.isolation.supports_named_processes.is_supported());
}
