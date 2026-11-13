use serde_json::{Value, json};

use super::*;

fn entries(lines: &[&str]) -> Vec<u8> {
    let mut text = lines.join("\n");
    text.push('\n');
    text.into_bytes()
}

fn counter() -> impl FnMut() -> String {
    let mut next = 0;
    move || {
        next += 1;
        format!("id-{next}")
    }
}

fn run(state: &TranscriptState, up_to: Option<&str>, title: Option<&str>) -> Result<ForkedTranscript, ForkTsError> {
    let mut ids = counter();
    let mut clock = || 1_700_000_000_123_i64;
    fork_entries(
        state,
        &ForkTsRequest {
            session_id: "s0",
            up_to_message_id: up_to,
            title,
        },
        || "Derived".to_owned(),
        &mut ids,
        &mut clock,
    )
}

#[test]
fn js_stringify_prints_numbers_like_javascript() {
    assert_eq!(js_stringify(&json!(1.0)), "1");
    assert_eq!(js_stringify(&json!(1.5)), "1.5");
    assert_eq!(js_stringify(&json!(1e21)), "1e+21");
    assert_eq!(js_stringify(&json!(1e-7)), "1e-7");
    assert_eq!(js_stringify(&json!(0.000001)), "0.000001");
    assert_eq!(js_stringify(&json!(-0.0)), "0");
    assert_eq!(js_stringify(&json!(123456789012345680000.0)), "123456789012345680000");
}

#[test]
fn loader_skips_blank_and_malformed_lines() {
    let content = entries(&[
        "  ",
        "not json",
        r#"{"type":"user","uuid":"a","sessionId":"s0"}"#,
        r#"{"type":"relocated","sessionId":"s0","relocatedCwd":"/x"}"#,
    ]);
    let state = load_state(&content, "s0");
    assert_eq!(state.transcript.len(), 1);
    assert_eq!(state.relocated_cwd.as_deref(), Some("/x"));
}

#[test]
fn fork_rewrites_ids_parents_and_appends_title() -> Result<(), ForkTsError> {
    let content = entries(&[
        r#"{"type":"user","uuid":"a","parentUuid":null,"sessionId":"s0","timestamp":"t1","message":{"role":"user","content":"hi"}}"#,
        r#"{"type":"assistant","uuid":"b","parentUuid":"a","sessionId":"s0","timestamp":"t2","message":{"role":"assistant","content":"yo"}}"#,
    ]);
    let state = load_state(&content, "s0");
    let forked = run(&state, None, None)?;
    assert_eq!(forked.session_id, "id-3");
    let values: Vec<Value> = forked.entries.iter().cloned().map(Value::Object).collect();
    assert_eq!(values[0]["uuid"], "id-1");
    assert_eq!(values[0]["parentUuid"], Value::Null);
    assert_eq!(values[1]["parentUuid"], "id-1");
    assert_eq!(values[1]["forkedFrom"]["messageUuid"], "b");
    assert_eq!(values[1]["timestamp"], "2023-11-14T22:13:20.123Z");
    assert_eq!(values[2]["customTitle"], "Derived (fork)");
    Ok(())
}

#[test]
fn fork_up_to_unknown_message_fails() {
    let content = entries(&[r#"{"type":"user","uuid":"a","sessionId":"s0"}"#]);
    let state = load_state(&content, "s0");
    let error = run(&state, Some("zzz"), None).err();
    assert_eq!(
        error,
        Some(ForkTsError::MessageNotFound {
            message_id: "zzz".to_owned(),
            session_id: "s0".to_owned()
        })
    );
}

#[test]
fn fork_of_empty_transcript_fails() {
    let state = load_state(b"", "s0");
    assert_eq!(
        run(&state, None, None).err(),
        Some(ForkTsError::NoMessages("s0".to_owned()))
    );
}

macro_rules! oracle_case {
    ($name:ident, $up_to:expr, $title:expr) => {
        #[test]
        fn $name() -> Result<(), ForkTsError> {
            let input = include_bytes!(concat!(
                "../tests/fixtures/fork_ts/",
                stringify!($name),
                ".input.jsonl"
            ));
            let expected = include_str!(concat!(
                "../tests/fixtures/fork_ts/",
                stringify!($name),
                ".expected.jsonl"
            ));
            let state = load_state(input, "s0");
            let forked = run(&state, $up_to, $title)?;
            assert_eq!(serialize_entries(&forked.entries), expected);
            Ok(())
        }
    };
}

// Expected bytes come from running the pinned 0.3.323 bundle functions (loader plus `NB`) in a node VM
// with counter ids and a fixed clock on the same input files.
oracle_case!(chain_basics, None, None);
oracle_case!(compaction, None, None);
oracle_case!(data_flow_derived_from, None, None);
oracle_case!(compaction_cut_at_a2, Some("a2"), None);
oracle_case!(compaction_cut_at_u3_titled, Some("u3"), Some("Mine"));
oracle_case!(compaction_cut_at_cb, Some("cb"), None);
oracle_case!(chain_cut_at_first, Some("u1"), None);
