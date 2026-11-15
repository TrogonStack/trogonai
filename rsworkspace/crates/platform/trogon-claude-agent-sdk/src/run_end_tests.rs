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
    RunEnd::new(style, Some(Duration::from_secs(60)))
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
    run.finish(false, true, None);
    run.observe(&task("task_started", "t"), true);
    run.observe(&state("running"), true);
    run.observe(&result(), true);
    assert!(!run.close_due());
    assert!(run.deadline().is_some());
    run.observe(&state("idle"), true);
    assert!(run.close_due());

    let mut run = machine(ArgvStyle::TypeScript);
    run.prompt_written();
    run.finish(false, true, None);
    run.observe(&task("task_started", "t"), true);
    run.observe(&result(), true);
    assert!(run.close_due());
}

/// `streamInput`'s `count` and `stream_input`'s `written` belong to the call: one that wrote nothing closes stdin
/// at once, whatever the session wrote before it.
#[test]
fn an_input_that_wrote_nothing_closes_at_once() {
    for style in [ArgvStyle::TypeScript, ArgvStyle::Python] {
        let mut run = machine(style);
        run.prompt_written();
        run.finish(false, true, Some(0));
        assert!(run.close_due());

        let mut run = machine(style);
        run.prompt_written();
        run.finish(false, true, Some(1));
        assert!(!run.close_due());
    }
}

#[test]
fn typescript_single_turn_closes_inside_the_run_end() {
    let mut run = machine(ArgvStyle::TypeScript);
    run.prompt_written();
    run.finish(true, false, None);
    assert!(!run.close_due());
    run.observe(&result(), false);
    assert!(run.close_due());

    let mut run = machine(ArgvStyle::TypeScript);
    run.prompt_written();
    run.finish(true, true, None);
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
        run.finish(false, false, None);
        assert!(run.close_due());
        let mut run = machine(style);
        run.finish(false, true, None);
        assert!(run.close_due());
        let mut run = machine(style);
        run.prompt_written();
        run.finish(false, true, None);
        assert!(!run.close_due());
    }
}

/// Python: tasks in flight hold the run open at a result or idle, and the ceiling leaves them alone.
#[test]
fn python_keeps_the_run_open_while_a_tracked_task_is_in_flight() {
    let mut run = machine(ArgvStyle::Python);
    run.prompt_written();
    run.finish(false, true, None);
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
    run.finish(false, true, None);
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

fn ms(style: ArgvStyle, raw: Option<&str>) -> Option<u64> {
    match style {
        ArgvStyle::TypeScript => crate::run_end_ceiling::typescript(raw),
        ArgvStyle::Python => crate::run_end_ceiling::python(raw),
    }
    .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

#[test]
fn the_typescript_ceiling_is_read_as_gd_and_j7_read_it() {
    let read = |raw| ms(ArgvStyle::TypeScript, Some(raw));
    assert_eq!(ms(ArgvStyle::TypeScript, None), Some(600_000));
    assert_eq!(read("0"), None);
    assert_eq!(read("-5"), Some(600_000));
    assert_eq!(read("5e2"), Some(500));
    assert_eq!(read("1.5e3"), Some(1500));
    assert_eq!(read("500ms"), Some(500));
    assert_eq!(read(" 500 "), Some(500));
    assert_eq!(read("1,000"), Some(1000));
    assert_eq!(read("1 000 000"), Some(1_000_000));
    assert_eq!(read("abc"), Some(600_000));
    assert_eq!(read(""), Some(600_000));
    assert_eq!(read("1e400"), Some(600_000));
    assert_eq!(read("1.5e0"), Some(600_000));
    assert_eq!(read("99999999999"), Some((1 << 31) - 1));
}

#[test]
fn the_python_ceiling_is_read_as_int_reads_it() {
    let read = |raw| ms(ArgvStyle::Python, Some(raw));
    assert_eq!(ms(ArgvStyle::Python, None), Some(600_000));
    assert_eq!(read("0"), None);
    assert_eq!(read("-0"), None);
    assert_eq!(read("+7"), Some(7));
    assert_eq!(read(" 500 "), Some(500));
    assert_eq!(read("1_000"), Some(1000));
    assert_eq!(read("-5"), Some(600_000));
    assert_eq!(read("5e2"), Some(600_000));
    assert_eq!(read("500ms"), Some(600_000));
    assert_eq!(read("1,000"), Some(600_000));
    assert_eq!(read("99999999999"), Some((1 << 31) - 1));
}

#[test]
fn a_disabled_ceiling_never_arms_the_deadline() {
    for style in [ArgvStyle::TypeScript, ArgvStyle::Python] {
        let mut run = RunEnd::new(style, None);
        run.observe(&result(), true);
        run.observe(&state("running"), true);
        assert!(run.deadline().is_none());
    }
}

/// Python `_run_final`: set when the input waiter closes stdin (clearing the ceiling when something was written),
/// and when the reader is gone; read by the reopening and by the ceiling.
#[test]
fn python_final_state_is_set_and_read_where_query_py_does() {
    let mut run = machine(ArgvStyle::Python);
    run.prompt_written();
    run.finish(false, true, None);
    run.observe(&state("running"), true);
    run.observe(&result(), true);
    assert!(run.deadline().is_some());
    run.closing_input();
    assert!(run.deadline().is_none());
    run.observe(&state("running"), true);
    assert!(run.deadline().is_none());

    let mut run = machine(ArgvStyle::Python);
    run.finish(false, true, None);
    run.observe(&state("running"), true);
    run.observe(&result(), true);
    assert!(run.deadline().is_some());
    run.closing_input();
    assert!(run.deadline().is_some());
    run.observe(&state("running"), true);
    assert!(run.deadline().is_none());

    let mut run = machine(ArgvStyle::Python);
    run.prompt_written();
    run.finish(false, true, None);
    run.reader_gone();
    assert!(run.close_due());
    run.observe(&assistant(), true);
    run.observe(&state("running"), true);
    run.observe(&result(), true);
    assert!(run.deadline().is_none());
}
