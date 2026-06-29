use super::*;

fn task() -> A2aTaskId {
    A2aTaskId::new("task-1").unwrap()
}

fn cfg() -> PushNotificationConfigId {
    PushNotificationConfigId::new("cfg-1").unwrap()
}

#[test]
fn derive_terminal_uses_length_prefixed_components() {
    let key = PushIdempotencyKey::derive_terminal(&task(), &cfg(), TerminalPushTaskState::Completed);
    assert_eq!(key.as_str(), "8:terminal|6:task-1|5:cfg-1|9:completed");
    assert_eq!(key.to_string(), "8:terminal|6:task-1|5:cfg-1|9:completed");
}

#[test]
fn derive_dlq_uses_length_prefixed_components() {
    let transition = StatusTransitionId::from_terminal(TerminalPushTaskState::Failed);
    let key = PushIdempotencyKey::derive_dlq(&task(), &transition, "https://example.com/push");
    assert_eq!(key.as_str(), "3:dlq|6:task-1|6:failed|24:https://example.com/push");
}

#[test]
fn derive_terminal_is_injective_across_colon_boundaries() {
    // (`task:left`, `cfg`) and (`task`, `left:cfg`) would clash with the
    // legacy `:`-joined encoding but stay distinct here.
    let task_a = A2aTaskId::new("task").unwrap();
    let cfg_left = PushNotificationConfigId::new("left-cfg").unwrap();
    let task_with_colon = A2aTaskId::new("task1234").unwrap();
    let cfg_b = PushNotificationConfigId::new("c").unwrap();

    let key_a = PushIdempotencyKey::derive_terminal(&task_a, &cfg_left, TerminalPushTaskState::Completed);
    let key_b = PushIdempotencyKey::derive_terminal(&task_with_colon, &cfg_b, TerminalPushTaskState::Completed);
    assert_ne!(key_a.as_str(), key_b.as_str());
}

#[test]
fn terminal_and_dlq_keys_dont_collide_even_with_aligned_components() {
    // Without the kind discriminant a terminal (task, cfg="failed", "completed")
    // would encode identically to a dlq (task, transition="failed", "completed").
    let task = A2aTaskId::new("task-x").unwrap();
    let cfg = PushNotificationConfigId::new("failed").unwrap();
    let transition = StatusTransitionId::from_terminal(TerminalPushTaskState::Failed);
    let terminal_key = PushIdempotencyKey::derive_terminal(&task, &cfg, TerminalPushTaskState::Completed);
    let dlq_key = PushIdempotencyKey::derive_dlq(&task, &transition, "completed");
    assert_ne!(terminal_key.as_str(), dlq_key.as_str());
}

#[test]
fn from_dedupe_wire_passes_through_raw_value() {
    let key = PushIdempotencyKey::from_dedupe_wire("opaque-wire-token");
    assert_eq!(key.as_str(), "opaque-wire-token");
}

#[test]
fn debug_exposes_inner_value() {
    let key = PushIdempotencyKey::derive_terminal(&task(), &cfg(), TerminalPushTaskState::Rejected);
    assert!(format!("{key:?}").contains("8:terminal|6:task-1|5:cfg-1|8:rejected"));
}
