mod support;

use std::sync::Arc;

use serde_json::{Value, json};
use support::{MockSandboxBackend, Release, TestResult, WAIT, api_key, open_socket, read_next, send};
use trogon_sandbox_contract::{CancelOperationResponse, OperationId, OperationRecord, TenantId};

fn stored(tenant: &str, id: &str) -> TestResult<OperationRecord> {
    Ok(serde_json::from_value(json!({
        "id": id, "tenant": tenant, "kind": "create", "target": "actual-lease",
        "request_fingerprint": "stored-fingerprint", "state": "succeeded", "attempts": 1,
        "requested_at": {"secs_since_epoch": 100, "nanos_since_epoch": 0},
        "result": {
            "result": "granted", "lease_id": "actual-lease", "provider": "e2b",
            "sandbox_id": "actual-provider-sandbox", "evidence_subject": "sandbox.evidence.actual-lease",
            "deadline": {"secs_since_epoch": 1900000000, "nanos_since_epoch": 0},
            "created": true, "capabilities": []
        }
    }))?)
}

fn operation_id(value: &str) -> TestResult<OperationId> {
    Ok(OperationId::new(value)?)
}

#[tokio::test]
async fn pending_foreground_keeps_operation_capability_cancel_and_get_available_on_the_same_socket() -> TestResult {
    let backend = Arc::new(MockSandboxBackend::new());
    backend.set_operation_support(true);
    backend.set_operation_cancel_response(CancelOperationResponse::Pending);
    backend.record_operation(stored("owner", "operation-1")?);
    let (entered, release) = backend.pause_next_command();
    let _release = Release(release.clone());
    let mut socket = open_socket(backend.clone(), &api_key("operation-1"), "owner").await?;
    send(
        &mut socket,
        "blocked",
        "sandbox_run_command",
        json!({"lease_id": "owned-lease", "command": ["sleep", "60"]}),
    )
    .await?;
    tokio::time::timeout(WAIT, entered.notified()).await?;
    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "cap",
            "sandbox_operation_capabilities",
            json!({}),
            json!({"supports_create_cancellation": true}),
        ),
        (
            "cancel",
            "sandbox_cancel_operation",
            json!({"operation_id": "operation-1"}),
            json!({"outcome": "pending"}),
        ),
    ];
    for (id, tool, payload, expected) in cases {
        send(&mut socket, id, tool, payload).await?;
        let response = read_next(&mut socket).await?;
        assert_eq!(response["id"], id);
        assert_eq!(response["result"], expected);
    }
    send(
        &mut socket,
        "get",
        "sandbox_get_operation",
        json!({"operation_id": "operation-1"}),
    )
    .await?;
    let response = read_next(&mut socket).await?;
    assert_eq!(response["id"], "get");
    assert_eq!(response["result"]["outcome"], "found");
    assert_eq!(response["result"]["operation"]["result"]["lease_id"], "actual-lease");
    assert_eq!(
        backend.operation_calls(),
        vec![
            ("capabilities".to_string(), TenantId::new("owner")?, None),
            (
                "cancel".to_string(),
                TenantId::new("owner")?,
                Some(operation_id("operation-1")?)
            ),
            (
                "get".to_string(),
                TenantId::new("owner")?,
                Some(operation_id("operation-1")?)
            ),
        ]
    );
    release.notify_one();
    assert_eq!(read_next(&mut socket).await?["id"], "blocked");
    socket.close(None).await?;
    Ok(())
}

#[tokio::test]
async fn malformed_ids_and_tenant_overrides_are_rejected_before_operation_backend_dispatch() -> TestResult {
    let backend = Arc::new(MockSandboxBackend::new());
    let mut socket = open_socket(backend.clone(), &api_key("operation-2"), "owner").await?;
    let cases: Vec<(&str, Value)> = vec![
        ("sandbox_operation_capabilities", json!({"tenant": "foreign"})),
        (
            "sandbox_cancel_operation",
            json!({"operation_id": "operation-1", "tenant": "foreign"}),
        ),
        (
            "sandbox_get_operation",
            json!({"operation_id": "operation-1", "tenant": "foreign"}),
        ),
        ("sandbox_cancel_operation", json!({"operation_id": "../foreign"})),
        ("sandbox_get_operation", json!({"operation_id": ""})),
    ];
    for (index, (tool, payload)) in cases.into_iter().enumerate() {
        let id = format!("invalid-{index}");
        send(&mut socket, &id, tool, payload).await?;
        let response = read_next(&mut socket).await?;
        assert_eq!(response["id"], id.as_str());
        assert_eq!(response["error"]["code"], "invalid_payload");
    }
    assert!(backend.operation_calls().is_empty());
    socket.close(None).await?;
    Ok(())
}

#[tokio::test]
async fn a_foreign_stored_operation_is_not_disclosed_under_the_session_tenant() -> TestResult {
    let backend = Arc::new(MockSandboxBackend::new());
    backend.record_operation(stored("foreign", "operation-foreign")?);
    let mut socket = open_socket(backend.clone(), &api_key("operation-3"), "owner").await?;
    send(
        &mut socket,
        "foreign",
        "sandbox_get_operation",
        json!({"operation_id": "operation-foreign"}),
    )
    .await?;
    let response = read_next(&mut socket).await?;
    assert_eq!(response["result"], json!({"outcome": "not_found"}));
    assert_eq!(
        backend.operation_calls(),
        vec![(
            "get".to_string(),
            TenantId::new("owner")?,
            Some(operation_id("operation-foreign")?)
        )]
    );
    socket.close(None).await?;
    Ok(())
}

#[tokio::test]
async fn explicit_created_compensation_reaches_the_session_scoped_typed_backend() -> TestResult {
    let backend = Arc::new(MockSandboxBackend::new());
    backend.set_operation_cancel_response(CancelOperationResponse::Canceled);
    let mut socket = open_socket(backend.clone(), &api_key("operation-4"), "owner").await?;
    send(
        &mut socket,
        "compensate",
        "sandbox_cancel_operation",
        json!({"operation_id": "late-create", "compensate_created": true}),
    )
    .await?;
    let response = read_next(&mut socket).await?;
    assert_eq!(response["result"], json!({"outcome": "canceled"}));
    let requests = backend.operation_cancel_requests();
    assert_eq!(requests.len(), 1);
    let captured = serde_json::to_value(&requests[0])?;
    assert_eq!(captured["tenant"], "owner");
    assert_eq!(captured["operation_id"], "late-create");
    assert_eq!(captured["compensate_created"], true);
    socket.close(None).await?;
    Ok(())
}
