//! Error classification, kind tokens and provider/lease accessors.

use serde_json::{Value, json};
use trogon_sandbox_contract::{DriveError, FileOpErrorKind, LeaseError, LeaseId, ProviderKind, TerminalError};
use trogon_sandbox_protocol::{Classified, ErrorClass, ErrorKind, LeaseErrorSubject};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn lease_error_samples() -> Vec<(Value, bool)> {
    vec![
        (json!({"kind": "quota_exceeded", "tenant": "t"}), false),
        (json!({"kind": "no_provider_available"}), true),
        (json!({"kind": "invalid_spec", "reason": "r"}), false),
        (json!({"kind": "invalid_network_policy", "reason": "r"}), false),
        (json!({"kind": "backend", "message": "m"}), true),
        (json!({"kind": "unauthorized", "reason": "r"}), false),
        (
            json!({"kind": "no_worker_satisfies_capabilities", "requirements": {}}),
            false,
        ),
        (
            json!({"kind": "no_worker_can_restore_snapshot", "digest": DIGEST}),
            false,
        ),
        (
            json!({"kind": "too_many_active_leases", "tenant": "t", "cap": 1}),
            false,
        ),
        (
            json!({"kind": "too_many_tagged_leases", "tenant": "t", "tag_key": "user", "tag_value": "u", "cap": 2}),
            false,
        ),
        (json!({"kind": "snapshot_quota_exceeded", "tenant": "t"}), false),
        (
            json!({"kind": "snapshot_tag_not_found", "tag": {"tenant": "t", "name": "n"}}),
            false,
        ),
        (
            json!({"kind": "snapshot_unsupported", "provider": "mock", "reason": "r"}),
            false,
        ),
        (json!({"kind": "snapshot_signature_invalid", "digest": DIGEST}), false),
        (json!({"kind": "prewarm_quota_exceeded", "tenant": "t"}), false),
        (json!({"kind": "image_not_ready", "image": "i", "reason": "r"}), true),
        (json!({"kind": "source_lease_not_found", "lease_id": "l"}), false),
        (json!({"kind": "source_lease_not_running", "lease_id": "l"}), false),
        (
            json!({"kind": "checkpoint_running_unsupported", "provider": "mock"}),
            false,
        ),
        (json!({"kind": "checkpoint_running_failed", "reason": "r"}), true),
        (json!({"kind": "snapshots_disabled", "lease_id": "l"}), false),
        (json!({"kind": "lease_not_found", "lease_id": "l"}), false),
        (json!({"kind": "lease_not_running", "lease_id": "l"}), false),
        (json!({"kind": "invalid_tags", "reason": "r"}), false),
        (json!({"kind": "would_exceed_max_lifetime", "max": 1}), false),
        (
            json!({"kind": "provider_launch_failed", "provider": "mock", "reason": "r"}),
            true,
        ),
        (
            json!({"kind": "provider_rejected_dispatch", "provider": "mock", "reason": "r"}),
            true,
        ),
        (
            json!({"kind": "file_too_large_for_transport", "path": "p", "bytes": 1, "max_payload_bytes": 1}),
            false,
        ),
        (
            json!({"kind": "command_output_too_large_for_transport", "exit_code": 0, "stdout_bytes": 1, "stderr_bytes": 1, "max_payload_bytes": 1}),
            false,
        ),
        (
            json!({"kind": "files_too_large_for_transport", "content_bytes": 1, "max_payload_bytes": 1}),
            false,
        ),
        (json!({"kind": "name_conflict", "name": "n"}), true),
        (
            json!({"kind": "native_passthrough_not_permitted", "tenant": "t"}),
            false,
        ),
        (
            json!({"kind": "invalid_native_options", "provider": "mock", "reason": "r"}),
            false,
        ),
        (json!({"kind": "no_worker_in_region", "regions": ["eu"]}), true),
        (
            json!({"kind": "operation_conflict", "operation_id": "o", "reason": "r"}),
            false,
        ),
        (json!({"kind": "operation_not_found", "operation_id": "o"}), false),
        (json!({"kind": "operation_canceled", "operation_id": "o"}), false),
        (
            json!({"kind": "suspend_unsupported", "lease_id": "l", "requested": "disk"}),
            false,
        ),
        (
            json!({"kind": "lease_waking", "lease_id": "l", "retry_after_ms": 1000}),
            true,
        ),
        (json!({"kind": "suspend_artifact_lost", "lease_id": "l"}), false),
        (json!({"kind": "entity_unavailable", "reason": "store down"}), true),
        (json!({"kind": "entity_not_enabled"}), false),
    ]
}

fn tag_of(value: &Value) -> Result<String, Box<dyn std::error::Error>> {
    Ok(value["kind"].as_str().ok_or("sample has a kind")?.to_string())
}

#[test]
fn every_lease_error_variant_is_classified_with_a_matching_kind() -> TestResult {
    let mut seen = std::collections::BTreeSet::new();
    for (sample, retryable) in lease_error_samples() {
        let tag = tag_of(&sample)?;
        let error: LeaseError =
            serde_json::from_value(sample).map_err(|e| format!("sample for {tag} does not parse: {e}"))?;
        assert_eq!(error.kind(), tag, "kind() must equal the serde tag");
        let class = error.class();
        assert_eq!(class.retryable, retryable, "{tag}");
        assert!(!class.outcome_unknown, "{tag}");
        assert!(!class.hint.is_empty(), "{tag}");
        assert!(seen.insert(tag.clone()), "duplicate sample {tag}");
    }
    assert_eq!(seen.len(), 42);
    Ok(())
}

/// The TS client carries a fallback copy of the lease error table. This keeps its key set and retryable flags equal
/// to the Rust classifier.
#[test]
fn the_ts_client_table_matches_the_rust_classifier() -> TestResult {
    let ts = include_str!("../../../../../tsworkspace/packages/sandbox-ws-client/src/error-classes.ts");
    let block = ts
        .split("export const LEASE_ERROR_CLASSES")
        .nth(1)
        .and_then(|rest| rest.split("\n};").next())
        .ok_or("LEASE_ERROR_CLASSES block")?;
    let mut ts_table = std::collections::BTreeMap::new();
    for line in block.lines() {
        let Some((kind, rest)) = line.trim().split_once(": ") else {
            continue;
        };
        if rest.starts_with("permanent(") {
            ts_table.insert(kind.to_string(), false);
        } else if rest.starts_with("transient(") {
            ts_table.insert(kind.to_string(), true);
        }
    }
    let mut rust_table = std::collections::BTreeMap::new();
    for (sample, _) in lease_error_samples() {
        let error: LeaseError = serde_json::from_value(sample)?;
        rust_table.insert(error.kind().to_string(), error.class().retryable);
    }
    assert_eq!(ts_table, rust_table);
    Ok(())
}

#[test]
fn lease_error_exposes_the_provider_and_lease_it_names() -> TestResult {
    let error: LeaseError =
        serde_json::from_value(json!({"kind": "provider_launch_failed", "provider": "local_docker", "reason": "r"}))?;
    assert_eq!(error.provider(), Some(ProviderKind::LocalDocker));
    assert!(error.lease_id().is_none());
    let error = LeaseError::LeaseNotFound {
        lease_id: LeaseId::new("l-1"),
    };
    assert_eq!(error.lease_id(), Some(&LeaseId::new("l-1")));
    assert!(error.provider().is_none());
    Ok(())
}

#[test]
fn every_drive_error_variant_is_classified_with_a_matching_kind() -> TestResult {
    let drive = json!({"tenant": "t", "name": "d"});
    let samples = vec![
        (json!({"kind": "drive_not_found", "drive_id": drive}), false),
        (json!({"kind": "drive_already_exists", "drive_id": drive}), false),
        (
            json!({"kind": "drive_attached", "drive_id": drive, "lease_id": "l"}),
            false,
        ),
        (
            json!({"kind": "drive_size_exceeded", "requested_gib": 2, "max_gib": 1}),
            false,
        ),
        (json!({"kind": "too_many_drives", "count": 2, "max": 1}), false),
        (json!({"kind": "drive_mount_path_overlap", "mount_path": "/a"}), false),
        (
            json!({"kind": "invalid_drive_mount_path", "mount_path": "/a", "reason": "r"}),
            false,
        ),
        (json!({"kind": "storage_backend", "message": "m"}), true),
        (json!({"kind": "unauthorized", "reason": "r"}), false),
    ];
    assert_eq!(samples.len(), 9);
    for (sample, retryable) in samples {
        let tag = tag_of(&sample)?;
        let error: DriveError =
            serde_json::from_value(sample).map_err(|e| format!("sample for {tag} does not parse: {e}"))?;
        assert_eq!(error.kind(), tag);
        assert_eq!(error.class().retryable, retryable, "{tag}");
        assert!(!error.class().hint.is_empty(), "{tag}");
    }
    Ok(())
}

#[test]
fn every_terminal_error_variant_is_classified_with_a_matching_kind() -> TestResult {
    let samples = vec![
        (json!({"kind": "not_found"}), false),
        (json!({"kind": "id_in_use"}), false),
        (json!({"kind": "too_many_terminals", "max": 8}), false),
        (json!({"kind": "invalid_request", "reason": "r"}), false),
        (json!({"kind": "spawn_failed", "reason": "r"}), true),
        (json!({"kind": "exited"}), false),
    ];
    assert_eq!(samples.len(), 6);
    for (sample, retryable) in samples {
        let tag = tag_of(&sample)?;
        let error: TerminalError =
            serde_json::from_value(sample).map_err(|e| format!("sample for {tag} does not parse: {e}"))?;
        assert_eq!(error.kind(), tag);
        assert_eq!(error.class().retryable, retryable, "{tag}");
        assert!(!error.class().hint.is_empty(), "{tag}");
    }
    Ok(())
}

#[test]
fn every_file_op_error_kind_is_classified() {
    let kinds = [
        (FileOpErrorKind::InvalidPath, false),
        (FileOpErrorKind::PermissionDenied, false),
        (FileOpErrorKind::AlreadyExists, false),
        (FileOpErrorKind::NotADirectory, false),
        (FileOpErrorKind::IsADirectory, false),
        (FileOpErrorKind::DirectoryNotEmpty, false),
        (FileOpErrorKind::TooLarge, false),
        (FileOpErrorKind::InvalidRequest, false),
        (FileOpErrorKind::Io, true),
        (FileOpErrorKind::UnknownCheckpoint, false),
        (FileOpErrorKind::UnknownWatch, false),
    ];
    for (kind, retryable) in kinds {
        assert_eq!(kind.class().retryable, retryable, "{kind:?}");
        assert!(!kind.class().hint.is_empty(), "{kind:?}");
    }
}

#[test]
fn class_constructors_set_the_flags() {
    let permanent = ErrorClass::permanent("h");
    assert!(!permanent.retryable && !permanent.outcome_unknown);
    let transient = ErrorClass::transient("h");
    assert!(transient.retryable && !transient.outcome_unknown);
    let unknown = ErrorClass::unknown_outcome("h");
    assert!(unknown.retryable && unknown.outcome_unknown);
    assert_eq!(unknown.hint, "h");
}

#[test]
fn provider_and_lease_accessors_cover_exactly_the_variants_that_name_them() -> TestResult {
    let names_provider = [
        "snapshot_unsupported",
        "checkpoint_running_unsupported",
        "provider_launch_failed",
        "invalid_native_options",
        "provider_rejected_dispatch",
    ];
    let names_lease = [
        "source_lease_not_found",
        "source_lease_not_running",
        "lease_not_found",
        "lease_not_running",
        "suspend_unsupported",
        "lease_waking",
        "suspend_artifact_lost",
        "snapshots_disabled",
    ];
    for (sample, _) in lease_error_samples() {
        let tag = tag_of(&sample)?;
        let error: LeaseError = serde_json::from_value(sample)?;
        assert_eq!(
            error.provider().is_some(),
            names_provider.contains(&tag.as_str()),
            "{tag}"
        );
        assert_eq!(error.lease_id().is_some(), names_lease.contains(&tag.as_str()), "{tag}");
    }
    Ok(())
}
