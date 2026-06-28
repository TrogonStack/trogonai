use super::*;

fn table() -> Tier1ResourceTupleTable {
    Tier1ResourceTupleTable::bundled()
}

#[test]
fn derive_message_send_uses_agent_invoke() {
    let tuple = table()
        .derive(
            &Tier1A2aMethodSlug::new("message/send"),
            "planner",
            "pub",
            &serde_json::json!({}),
        )
        .expect("derive ok");
    assert_eq!(tuple.resource_type.as_str(), "agent");
    assert_eq!(tuple.resource_id.as_str(), "planner");
    assert_eq!(tuple.permission.as_str(), "invoke");
}

#[test]
fn derive_message_stream_also_uses_agent_invoke() {
    let tuple = table()
        .derive(
            &Tier1A2aMethodSlug::new("message/stream"),
            "planner",
            "pub",
            &serde_json::json!({}),
        )
        .expect("derive ok");
    assert_eq!(tuple.permission.as_str(), "invoke");
}

#[test]
fn derive_tasks_list_uses_discover() {
    let tuple = table()
        .derive(
            &Tier1A2aMethodSlug::new("tasks/list"),
            "planner",
            "pub",
            &serde_json::json!({}),
        )
        .expect("derive ok");
    assert_eq!(tuple.permission.as_str(), "discover");
}

#[test]
fn derive_agent_card_uses_agent_card_resource() {
    let tuple = table()
        .derive(
            &Tier1A2aMethodSlug::new("agent/card"),
            "planner",
            "publisher",
            &serde_json::json!({}),
        )
        .expect("derive ok");
    assert_eq!(tuple.resource_type.as_str(), "agent_card");
    assert_eq!(tuple.resource_id.as_str(), "publisher/planner");
}

#[test]
fn derive_tasks_get_requires_task_id() {
    let err = table()
        .derive(
            &Tier1A2aMethodSlug::new("tasks/get"),
            "planner",
            "pub",
            &serde_json::json!({}),
        )
        .expect_err("missing task_id must error");
    assert_eq!(err, Tier1DeriveError::MissingTaskId);
}

#[test]
fn derive_tasks_get_reads_task_id_from_params() {
    let tuple = table()
        .derive(
            &Tier1A2aMethodSlug::new("tasks/get"),
            "planner",
            "pub",
            &serde_json::json!({"id": "task-123"}),
        )
        .expect("derive ok");
    assert_eq!(tuple.resource_type.as_str(), "task");
    assert_eq!(tuple.resource_id.as_str(), "planner:task-123");
    assert_eq!(tuple.permission.as_str(), "read");
}

#[test]
fn derive_tasks_cancel_uses_cancel_permission() {
    let tuple = table()
        .derive(
            &Tier1A2aMethodSlug::new("tasks/cancel"),
            "planner",
            "pub",
            &serde_json::json!({"taskId": "t-7"}),
        )
        .expect("derive ok");
    assert_eq!(tuple.permission.as_str(), "cancel");
    assert_eq!(tuple.resource_id.as_str(), "planner:t-7");
}

#[test]
fn derive_tasks_push_config_uses_configure_push() {
    let tuple = table()
        .derive(
            &Tier1A2aMethodSlug::new("tasks/pushNotificationConfig/set"),
            "planner",
            "pub",
            &serde_json::json!({"task_id": "t-9"}),
        )
        .expect("derive ok");
    assert_eq!(tuple.permission.as_str(), "configure_push");
}

#[test]
fn derive_unknown_method_errors() {
    let err = table()
        .derive(
            &Tier1A2aMethodSlug::new("does/not/exist"),
            "planner",
            "pub",
            &serde_json::json!({}),
        )
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
    let slug = Tier1A2aMethodSlug::new("message/send");
    assert_eq!(slug.as_str(), "message/send");
}
