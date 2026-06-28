use super::*;

fn table() -> Tier1ResourceTupleTable {
    Tier1ResourceTupleTable::bundled()
}

fn slug(s: &str) -> Tier1A2aMethodSlug {
    Tier1A2aMethodSlug::new(s).expect("non-empty test method slug")
}

fn inputs(method: &str, params: serde_json::Value) -> Tier1DeriveInputs {
    Tier1DeriveInputs::from_jsonrpc_params(slug(method), "planner", "pub", &params)
}

#[test]
fn derive_message_send_uses_agent_invoke() {
    let tuple = table()
        .derive(&inputs("message/send", serde_json::json!({})))
        .expect("derive ok");
    assert_eq!(tuple.resource_type.as_str(), "agent");
    assert_eq!(tuple.resource_id.as_str(), "planner");
    assert_eq!(tuple.permission.as_str(), "invoke");
}

#[test]
fn derive_message_stream_also_uses_agent_invoke() {
    let tuple = table()
        .derive(&inputs("message/stream", serde_json::json!({})))
        .expect("derive ok");
    assert_eq!(tuple.permission.as_str(), "invoke");
}

#[test]
fn derive_tasks_list_uses_discover() {
    let tuple = table()
        .derive(&inputs("tasks/list", serde_json::json!({})))
        .expect("derive ok");
    assert_eq!(tuple.permission.as_str(), "discover");
}

#[test]
fn derive_agent_card_uses_agent_card_resource() {
    let inputs =
        Tier1DeriveInputs::from_jsonrpc_params(slug("agent/card"), "planner", "publisher", &serde_json::json!({}));
    let tuple = table().derive(&inputs).expect("derive ok");
    assert_eq!(tuple.resource_type.as_str(), "agent_card");
    assert_eq!(tuple.resource_id.as_str(), "publisher/planner");
}

#[test]
fn derive_tasks_get_requires_task_id() {
    let err = table()
        .derive(&inputs("tasks/get", serde_json::json!({})))
        .expect_err("missing task_id must error");
    assert_eq!(err, Tier1DeriveError::MissingTaskId);
}

#[test]
fn derive_tasks_get_reads_task_id_from_params() {
    let tuple = table()
        .derive(&inputs("tasks/get", serde_json::json!({"id": "task-123"})))
        .expect("derive ok");
    assert_eq!(tuple.resource_type.as_str(), "task");
    assert_eq!(tuple.resource_id.as_str(), "planner:task-123");
    assert_eq!(tuple.permission.as_str(), "read");
}

#[test]
fn derive_tasks_cancel_uses_cancel_permission() {
    let tuple = table()
        .derive(&inputs("tasks/cancel", serde_json::json!({"taskId": "t-7"})))
        .expect("derive ok");
    assert_eq!(tuple.permission.as_str(), "cancel");
    assert_eq!(tuple.resource_id.as_str(), "planner:t-7");
}

#[test]
fn derive_tasks_push_config_uses_configure_push() {
    let tuple = table()
        .derive(&inputs(
            "tasks/pushNotificationConfig/set",
            serde_json::json!({"task_id": "t-9"}),
        ))
        .expect("derive ok");
    assert_eq!(tuple.permission.as_str(), "configure_push");
}

#[test]
fn push_notification_config_get_prefers_task_id_over_config_id() {
    // Wire shape per A2A spec: the JSON-RPC params for the push-
    // notification-config getters carry `taskId` for the task and
    // `id` for the push-config row. Reading `id` first (as the
    // task-id helper used to) targeted the wrong resource at
    // SpiceDb. The boundary now prefers `taskId` for these methods.
    let inputs = Tier1DeriveInputs::from_jsonrpc_params(
        slug("tasks/pushNotificationConfig/get"),
        "planner",
        "pub",
        &serde_json::json!({"taskId": "task-9", "id": "push-config-42"}),
    );
    assert_eq!(inputs.task_id.as_deref(), Some("task-9"));

    let tuple = table().derive(&inputs).expect("derive ok");
    assert_eq!(tuple.resource_id.as_str(), "planner:task-9");
}

#[test]
fn push_notification_config_delete_prefers_task_id_over_config_id() {
    let inputs = Tier1DeriveInputs::from_jsonrpc_params(
        slug("tasks/pushNotificationConfig/delete"),
        "planner",
        "pub",
        &serde_json::json!({"taskId": "task-9", "id": "push-config-42"}),
    );
    assert_eq!(inputs.task_id.as_deref(), Some("task-9"));
}

#[test]
fn derive_unknown_method_errors() {
    let err = table()
        .derive(&inputs("does/not/exist", serde_json::json!({})))
        .expect_err("unknown method must error");
    assert_eq!(err, Tier1DeriveError::UnknownMethod);
}

#[test]
fn tier1_derive_error_renders_expected_messages() {
    assert_eq!(
        Tier1DeriveError::MissingTaskId.to_string(),
        "task id missing from params"
    );
    assert_eq!(
        Tier1DeriveError::UnknownMethod.to_string(),
        "no tier-1 resource tuple for method"
    );
}

#[test]
fn table_rows_expose_bundled_set() {
    let rows = table();
    assert_eq!(rows.rows().len(), 11);
}

#[test]
fn method_slug_round_trips_through_as_str() {
    let slug = slug("message/send");
    assert_eq!(slug.as_str(), "message/send");
}

#[test]
fn value_objects_reject_empty_and_whitespace() {
    assert_eq!(
        Tier1ResourceType::new("").unwrap_err(),
        Tier1ValueObjectError::EmptyResourceType
    );
    assert_eq!(
        Tier1ResourceType::new("   ").unwrap_err(),
        Tier1ValueObjectError::EmptyResourceType
    );
    assert_eq!(
        Tier1ResourceId::new("").unwrap_err(),
        Tier1ValueObjectError::EmptyResourceId
    );
    assert_eq!(
        Tier1Permission::new("").unwrap_err(),
        Tier1ValueObjectError::EmptyPermission
    );
    assert_eq!(
        Tier1A2aMethodSlug::new("").unwrap_err(),
        Tier1ValueObjectError::EmptyMethodSlug
    );
}

#[test]
fn shape_for_returns_none_for_unknown_method() {
    assert!(table().shape_for(&slug("does/not/exist")).is_none());
}
