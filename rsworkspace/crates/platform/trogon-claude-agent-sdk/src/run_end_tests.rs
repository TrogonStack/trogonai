use serde_json::json;

use super::*;
use crate::MessageDecoding;

fn message(value: serde_json::Value) -> Message {
    let raw = value.as_object().cloned().unwrap_or_default();
    crate::message_decode::typed(&raw, MessageDecoding::TypeScript)
}

fn result() -> Message {
    message(json!({
        "type": "result", "subtype": "success", "is_error": false, "duration_ms": 1, "duration_api_ms": 1,
        "num_turns": 1, "session_id": "s", "result": "ok",
    }))
}

fn state(state: &str) -> Message {
    message(
        json!({"type": "system", "subtype": "session_state_changed", "state": state, "session_id": "s", "uuid": "u"}),
    )
}

fn task(subtype: &str, id: &str) -> Message {
    message(json!({
        "type": "system", "subtype": subtype, "task_id": id, "task_type": "local_agent", "session_id": "s",
        "uuid": "u", "description": "d", "status": "completed", "output_file": "f", "summary": "x",
    }))
}

fn assistant() -> Message {
    message(json!({
        "type": "assistant", "session_id": "s", "parent_tool_use_id": null,
        "message": {"role": "assistant", "content": [], "model": "m"},
    }))
}

fn machine(style: ArgvStyle) -> RunEnd {
    RunEnd::new(style, Duration::from_secs(60))
}

#[test]
fn fixtures_decode_to_the_messages_they_name() {
    assert!(matches!(result(), Message::Result(_)));
    assert!(matches!(state("idle"), Message::System(_)));
    assert!(matches!(task("task_started", "t"), Message::System(_)));
    assert!(matches!(assistant(), Message::Assistant(_)));
}

/// `sdk.mjs`: a result ends the run when there is no session state or nothing needs the CLI, and otherwise
/// only `idle` does. Tasks and pending continuations play no part.
#[test]
fn typescript_ends_the_run_at_the_result_or_at_idle() {
    let mut run = machine(ArgvStyle::TypeScript);
    run.prompt_written();
    run.finish(false, true);
    run.observe(&task("task_started", "t"), true);
    run.observe(&state("running"), true);
    run.observe(&result(), true);
    assert!(!run.close_due());
    assert!(run.deadline().is_some());
    run.observe(&state("idle"), true);
    assert!(run.close_due());

    let mut run = machine(ArgvStyle::TypeScript);
    run.prompt_written();
    run.finish(false, true);
    run.observe(&task("task_started", "t"), true);
    run.observe(&result(), true);
    assert!(run.close_due());
}

#[test]
fn typescript_single_turn_closes_inside_the_run_end() {
    let mut run = machine(ArgvStyle::TypeScript);
    run.prompt_written();
    run.finish(true, false);
    assert!(!run.close_due());
    run.observe(&result(), false);
    assert!(run.close_due());

    let mut run = machine(ArgvStyle::TypeScript);
    run.prompt_written();
    run.finish(true, true);
    run.observe(&state("running"), true);
    run.observe(&result(), true);
    assert!(!run.close_due());
    run.ceiling_elapsed();
    assert!(run.close_due());
}

#[test]
fn typescript_new_turn_work_reopens_a_streamed_run_and_stops_the_ceiling() {
    let mut run = machine(ArgvStyle::TypeScript);
    run.prompt_written();
    run.observe(&state("running"), true);
    run.observe(&result(), true);
    assert!(run.deadline().is_some());
    run.observe(&assistant(), true);
    assert!(run.deadline().is_none());
    run.observe(&state("requires_action"), true);
    assert!(run.deadline().is_none());
    run.observe(&state("running"), true);
    assert!(run.deadline().is_some());
}

#[test]
fn a_streamed_input_closes_when_written_unless_something_needs_the_cli() {
    for style in [ArgvStyle::TypeScript, ArgvStyle::Python] {
        let mut run = machine(style);
        run.prompt_written();
        run.finish(false, false);
        assert!(run.close_due());
        let mut run = machine(style);
        run.finish(false, true);
        assert!(run.close_due());
        let mut run = machine(style);
        run.prompt_written();
        run.finish(false, true);
        assert!(!run.close_due());
    }
}

/// Python: tasks in flight hold the run open at a result or idle, and the ceiling leaves them alone.
#[test]
fn python_keeps_the_run_open_while_a_tracked_task_is_in_flight() {
    let mut run = machine(ArgvStyle::Python);
    run.prompt_written();
    run.finish(false, true);
    run.observe(&task("task_started", "t"), true);
    run.observe(&result(), true);
    assert!(!run.close_due());
    run.observe(&state("idle"), true);
    assert!(!run.close_due());
    run.observe(&task("task_notification", "t"), true);
    run.observe(&result(), true);
    assert!(run.close_due());

    let mut run = machine(ArgvStyle::Python);
    run.prompt_written();
    run.finish(false, true);
    run.observe(&task("task_started", "t"), true);
    run.observe(&state("running"), true);
    run.observe(&result(), true);
    assert!(run.deadline().is_some());
    run.ceiling_elapsed();
    assert!(!run.close_due());
    run.observe(&task("task_notification", "t"), true);
    assert!(run.deadline().is_some());
    run.ceiling_elapsed();
    assert!(run.close_due());
}

#[test]
fn python_does_not_arm_the_ceiling_during_a_turn_or_without_needs() {
    let mut run = machine(ArgvStyle::Python);
    run.prompt_written();
    run.observe(&state("running"), true);
    run.observe(&assistant(), true);
    run.observe(&result(), true);
    assert!(run.deadline().is_some());
    run.observe(&assistant(), true);
    assert!(run.deadline().is_none());
    run.observe(&state("running"), true);
    assert!(run.deadline().is_none());
}
