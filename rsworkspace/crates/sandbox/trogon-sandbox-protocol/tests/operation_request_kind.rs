use serde_json::{Value, json};
use trogon_sandbox_contract::{OperationKind, OperationRequest};
use trogon_sandbox_protocol::operation_request_kind;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn request(image: Value) -> Value {
    json!({
        "tenant": "acme",
        "provider_pref": ["mock"],
        "spec": {
            "image": image,
            "workspace": { "empty": {} },
            "cpu_seconds": 1,
            "memory_mib": 64,
        },
        "ttl_seconds": 60,
        "idempotency_key": "k",
    })
}

fn create(image: Value) -> Result<OperationRequest, serde_json::Error> {
    serde_json::from_value(json!({ "kind": "create", "request": request(image), "attempt_id": "a1" }))
}

fn simple(kind: Value) -> Result<OperationRequest, serde_json::Error> {
    serde_json::from_value(kind)
}

#[test]
fn a_create_reports_restore_only_for_a_snapshot_image() -> TestResult {
    let registry = json!({ "kind": "registry", "reference": { "registry": "", "repository": "img" } });
    assert_eq!(operation_request_kind(&create(registry)?), OperationKind::Create);
    let snapshot = json!({
        "kind": "snapshot",
        "reference": {
            "registry": "r",
            "repository": "s",
            "digest": format!("sha256:{}", "a".repeat(64)),
            "artifact_type": "sandbox_rootfs_v1",
        },
    });
    assert_eq!(operation_request_kind(&create(snapshot)?), OperationKind::Restore);
    Ok(())
}

#[test]
fn every_other_request_maps_to_its_own_kind() -> TestResult {
    let snapshot = json!({
        "registry": "r",
        "repository": "s",
        "digest": format!("sha256:{}", "a".repeat(64)),
        "artifact_type": "sandbox_rootfs_v1",
    });
    for (wire, kind) in [
        (json!({ "kind": "pause" }), OperationKind::Pause),
        (json!({ "kind": "resume" }), OperationKind::Resume),
        (json!({ "kind": "suspend" }), OperationKind::Suspend),
        (json!({ "kind": "wake" }), OperationKind::Wake),
        (json!({ "kind": "delete" }), OperationKind::Delete),
        (
            json!({ "kind": "rollback", "snapshot": snapshot }),
            OperationKind::Rollback,
        ),
    ] {
        assert_eq!(operation_request_kind(&simple(wire)?), kind);
    }
    Ok(())
}
