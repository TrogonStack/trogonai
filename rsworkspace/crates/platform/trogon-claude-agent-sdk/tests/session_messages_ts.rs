#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Fixture transcripts are exact upstream-shaped JSONL lines."
    )
)]

use std::{error::Error, fs, path::Path};

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    ChainAlgorithm, MessageOptions, MessagePage, SessionFiles, SessionMessage, SessionMessageKind,
};

type TestResult = Result<(), Box<dyn Error>>;

const SESSION: &str = "123e4567-e89b-42d3-a456-426614174000";

fn jsonl(lines: &[Value]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

fn setup(lines: &[Value]) -> Result<(tempfile::TempDir, SessionFiles), Box<dyn Error>> {
    let config = tempfile::tempdir()?;
    let dir = config.path().join("projects").join("-proj");
    fs::create_dir_all(&dir)?;
    write_session(&dir, lines)?;
    let files = SessionFiles::with_config_dir(config.path());
    Ok((config, files))
}

fn write_session(dir: &Path, lines: &[Value]) -> Result<(), Box<dyn Error>> {
    fs::write(dir.join(format!("{SESSION}.jsonl")), jsonl(lines))?;
    Ok(())
}

fn user(uuid: &str, parent: Option<&str>, text: &str) -> Value {
    json!({"type":"user","uuid":uuid,"parentUuid":parent,"sessionId":SESSION,
           "timestamp":"2024-01-01T00:00:00.000Z","message":{"role":"user","content":text}})
}

fn assistant(uuid: &str, parent: Option<&str>, message_id: &str) -> Value {
    json!({"type":"assistant","uuid":uuid,"parentUuid":parent,"sessionId":SESSION,
           "timestamp":"2024-01-01T00:00:01.000Z",
           "message":{"id":message_id,"role":"assistant","content":[{"type":"text","text":"ok"}]}})
}

fn uuids(messages: &[SessionMessage]) -> Vec<&str> {
    messages.iter().map(|message| message.uuid.as_str()).collect()
}

fn options(include_system_messages: bool, algorithm: ChainAlgorithm) -> MessageOptions {
    MessageOptions {
        page: MessagePage::default(),
        include_system_messages,
        algorithm,
    }
}

fn compacted_session() -> Vec<Value> {
    let mut summary = user("s", Some("b"), "summary");
    summary["isCompactSummary"] = json!(true);
    vec![
        user("u1", None, "first"),
        assistant("a1", Some("u1"), "m1"),
        user("u2", Some("a1"), "kept"),
        assistant("a2", Some("u2"), "m2"),
        json!({"type":"system","subtype":"compact_boundary","uuid":"b","parentUuid":null,"sessionId":SESSION,
               "compactMetadata":{"preservedSegment":{"headUuid":"u2","anchorUuid":"s","tailUuid":"a2"}}}),
        summary,
        user("u3", Some("a2"), "after"),
    ]
}

#[test]
fn a_compact_boundary_relinks_the_preserved_segment_under_the_summary() -> TestResult {
    let (_config, files) = setup(&compacted_session())?;
    let typescript = files.get_session_messages_with(SESSION, None, options(false, ChainAlgorithm::TypeScript))?;
    assert_eq!(uuids(&typescript), ["s", "u2", "a2", "u3"]);
    assert!(typescript[0].extras.is_meta, "a compact summary is reported as meta");
    assert!(typescript[0].extras.is_compact_summary);
    let python = files.get_session_messages_with(SESSION, None, options(false, ChainAlgorithm::Python))?;
    assert_eq!(uuids(&python), ["u1", "a1", "u2", "a2", "u3"]);
    Ok(())
}

#[test]
fn preserved_messages_are_chained_after_the_anchor_and_later_children_move_to_the_last_one() -> TestResult {
    let mut summary = user("s", Some("b"), "summary");
    summary["isCompactSummary"] = json!(true);
    let (_config, files) = setup(&[
        user("u1", None, "first"),
        assistant("a1", Some("u1"), "m1"),
        user("u2", Some("a1"), "kept"),
        assistant("a2", Some("u1"), "m2"),
        json!({"type":"system","subtype":"compact_boundary","uuid":"b","parentUuid":null,"sessionId":SESSION,
               "compactMetadata":{"preservedMessages":{"anchorUuid":"s","uuids":["u2","a2"]}}}),
        summary,
        user("u3", Some("s"), "after"),
    ])?;
    let messages = files.get_session_messages(SESSION, None, MessagePage::default())?;
    assert_eq!(uuids(&messages), ["s", "u2", "a2", "u3"]);
    Ok(())
}

#[test]
fn system_entries_are_only_returned_on_request() -> TestResult {
    let (_config, files) = setup(&compacted_session())?;
    let messages = files.get_session_messages_with(SESSION, None, options(true, ChainAlgorithm::TypeScript))?;
    assert_eq!(uuids(&messages), ["b", "s", "u2", "a2", "u3"]);
    assert_eq!(messages[0].kind, SessionMessageKind::System);
    Ok(())
}

#[test]
fn a_sidechain_tip_never_becomes_the_leaf() -> TestResult {
    let mut side = assistant("side", Some("a1"), "m9");
    side["isSidechain"] = json!(true);
    let (_config, files) = setup(&[user("u1", None, "q"), assistant("a1", Some("u1"), "m1"), side])?;
    let messages = files.get_session_messages(SESSION, None, MessagePage::default())?;
    assert_eq!(uuids(&messages), ["u1", "a1"]);
    Ok(())
}

#[test]
fn a_delivered_queued_command_becomes_the_user_message_it_carried() -> TestResult {
    let (_config, files) = setup(&[
        user("u1", None, "q"),
        assistant("a1", Some("u1"), "m1"),
        json!({"type":"attachment","uuid":"q1","parentUuid":"a1","sessionId":SESSION,
               "timestamp":"2024-01-01T00:00:02.000Z",
               "attachment":{"type":"queued_command","prompt":"later","source_uuid":"src1"}}),
        assistant("a2", Some("q1"), "m2"),
    ])?;
    let messages = files.get_session_messages(SESSION, None, MessagePage::default())?;
    assert_eq!(uuids(&messages), ["u1", "a1", "src1", "a2"]);
    assert_eq!(messages[2].kind, SessionMessageKind::User);
    assert_eq!(messages[2].message, json!({"role":"user","content":"later"}));
    assert!(messages[2].extras.is_queued_command);
    Ok(())
}

#[test]
fn a_trailing_queued_command_without_a_reply_is_kept_as_an_attachment_unless_it_hangs_off_the_leaf() -> TestResult {
    let (_config, files) = setup(&[
        user("u1", None, "q"),
        assistant("a1", Some("u1"), "m1"),
        json!({"type":"attachment","uuid":"q1","parentUuid":"a1","sessionId":SESSION,
               "attachment":{"type":"queued_command","prompt":"pending"}}),
    ])?;
    let messages = files.get_session_messages(SESSION, None, MessagePage::default())?;
    assert_eq!(uuids(&messages), ["u1", "a1", "q1"]);
    assert_eq!(messages[2].message["content"], "pending");
    Ok(())
}

#[test]
fn local_command_records_and_outputs_are_marked_completed_and_the_caveat_is_hidden() -> TestResult {
    let mut caveat = user(
        "c1",
        Some("a1"),
        "<local-command-caveat>do not respond</local-command-caveat>",
    );
    caveat["isMeta"] = json!(true);
    let (_config, files) = setup(&[
        user("u1", None, "q"),
        assistant("a1", Some("u1"), "m1"),
        caveat,
        user("c2", Some("c1"), "<command-name>/status</command-name>"),
        user("c3", Some("c2"), "<local-command-stdout>ready</local-command-stdout>"),
    ])?;
    let messages = files.get_session_messages(SESSION, None, MessagePage::default())?;
    assert_eq!(uuids(&messages), ["u1", "a1", "c2", "c3"]);
    assert!(messages[2].extras.is_completed_local_command);
    assert!(messages[3].extras.is_completed_local_command);
    Ok(())
}

#[test]
fn a_voice_call_system_line_is_read_as_a_user_message() -> TestResult {
    let (_config, files) = setup(&[
        user("u1", None, "q"),
        assistant("a1", Some("u1"), "m1"),
        json!({"type":"system","subtype":"voice_call_talk","uuid":"v1","parentUuid":"a1","sessionId":SESSION,
               "content":"hello there","origin":{"kind":"voice-call"}}),
    ])?;
    let messages = files.get_session_messages(SESSION, None, MessagePage::default())?;
    assert_eq!(uuids(&messages), ["u1", "a1", "v1"]);
    assert_eq!(messages[2].message, json!({"role":"user","content":"hello there"}));
    assert_eq!(messages[2].extras.origin, Some(json!({"kind":"voice-call"})));
    Ok(())
}

#[test]
fn paging_applies_after_the_conversation_is_rebuilt() -> TestResult {
    let (_config, files) = setup(&compacted_session())?;
    let page = MessagePage {
        limit: Some(2),
        offset: 1,
    };
    let messages = files.get_session_messages_with(
        SESSION,
        None,
        MessageOptions {
            page,
            ..MessageOptions::default()
        },
    )?;
    assert_eq!(uuids(&messages), ["u2", "a2"]);
    Ok(())
}

#[test]
fn timestamps_and_denial_metadata_are_passed_through() -> TestResult {
    let mut result = json!({"type":"user","uuid":"r1","parentUuid":"a1","sessionId":SESSION,
        "timestamp":"2024-01-01T00:00:05.000Z","toolDenialKind":"user-rejected","toolDenialUnanswered":"stream-closed",
        "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call1","content":"no","is_error":true}]}});
    result["interruptedByShutdown"] = json!(true);
    let (_config, files) = setup(&[user("u1", None, "q"), assistant("a1", Some("u1"), "m1"), result])?;
    let messages = files.get_session_messages(SESSION, None, MessagePage::default())?;
    let last = &messages[2];
    assert_eq!(last.extras.timestamp, Some(json!("2024-01-01T00:00:05.000Z")));
    assert_eq!(last.extras.tool_denial_unanswered.as_deref(), Some("stream-closed"));
    assert_eq!(
        last.extras.tool_result_meta,
        Some(json!([{"id":"call1","non_execution_kind":"user-rejected"}]))
    );
    assert!(last.extras.interrupted_by_shutdown);
    Ok(())
}

fn boundary_with(metadata: Value) -> Vec<Value> {
    vec![
        user("u1", None, "first"),
        json!({"type":"system","subtype":"compact_boundary","uuid":"b","parentUuid":null,"sessionId":SESSION,
               "compactMetadata":metadata}),
    ]
}

#[test]
fn preserved_messages_without_a_uuids_list_fail_with_the_typescript_type_error() -> TestResult {
    let cases = [
        (
            json!({"preservedMessages":{"anchorUuid":"s"}}),
            "Cannot read properties of undefined (reading 'length')",
        ),
        (
            json!({"preservedMessages":{"anchorUuid":"s","uuids":null}}),
            "Cannot read properties of null (reading 'length')",
        ),
        (
            json!({"preservedMessages":true}),
            "Cannot read properties of undefined (reading 'length')",
        ),
        (
            json!({"preservedMessages":{"uuids":"abc"}}),
            "E.uuids.some is not a function",
        ),
        (
            json!({"preservedMessages":{"uuids":5}}),
            "E.uuids.some is not a function",
        ),
    ];
    for (metadata, message) in cases {
        let (_config, files) = setup(&boundary_with(metadata))?;
        let error = files
            .get_session_messages_with(SESSION, None, MessageOptions::default())
            .err()
            .ok_or("the pipeline should have thrown")?;
        assert_eq!(error.to_string(), message);
        let default_error = files
            .get_session_messages(SESSION, None, MessagePage::default())
            .err()
            .ok_or("the default getter must surface the throw")?;
        assert_eq!(default_error.to_string(), message);
        let python = files.get_session_messages_with(SESSION, None, options(false, ChainAlgorithm::Python))?;
        assert_eq!(uuids(&python), ["u1"], "the Python reader never fails");
    }
    Ok(())
}

#[test]
fn preserved_messages_that_do_not_throw_leave_the_transcript_alone() -> TestResult {
    for metadata in [
        json!({"preservedMessages":{"uuids":[]}}),
        json!({"preservedMessages":{"uuids":""}}),
        json!({"preservedMessages":{"uuids":["nope"]}}),
        json!({"preservedMessages":0}),
        json!({"preservedSegment":true}),
    ] {
        let (_config, files) = setup(&boundary_with(metadata))?;
        let messages = files.get_session_messages_with(SESSION, None, MessageOptions::default())?;
        assert_eq!(uuids(&messages), ["u1"]);
    }
    Ok(())
}

#[test]
fn the_python_dialect_uses_any_string_sidecar_parent_ids_as_python_does_and_typescript_validates_them() -> TestResult {
    let (config, files) = setup(&[user("u1", None, "hi")])?;
    let subagents = config
        .path()
        .join("projects")
        .join("-proj")
        .join(SESSION)
        .join("subagents");
    fs::create_dir_all(&subagents)?;
    fs::write(
        subagents.join("agent-beta.jsonl"),
        jsonl(&[user("s1", None, "task"), assistant("s2", Some("s1"), "m")]),
    )?;
    fs::write(
        subagents.join("agent-beta.meta.json"),
        json!({"toolUseId":"not valid!","parentAgentId":"bad.id"}).to_string(),
    )?;
    let python = files.get_subagent_messages_with(SESSION, "beta", None, options(false, ChainAlgorithm::Python));
    assert_eq!(python.len(), 2);
    assert_eq!(python[0].parent_tool_use_id.as_deref(), Some("not valid!"));
    assert_eq!(python[0].parent_agent_id.as_deref(), Some("bad.id"));
    let typescript =
        files.get_subagent_messages_with(SESSION, "beta", None, options(false, ChainAlgorithm::TypeScript));
    assert_eq!(typescript.len(), 2);
    assert_eq!(typescript[0].parent_tool_use_id, None);
    assert_eq!(typescript[0].parent_agent_id, None);
    Ok(())
}
