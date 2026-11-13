#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Upstream-shaped fixtures are written as JSON literals.")
)]

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use trogon_claude_agent_sdk::upstream::tool_inputs::{
    AgentInput, AgentInputEffort, AgentInputModel, AskUserQuestionInput, EnterPlanModeInput, GrepInput,
    GrepInputOutputMode, McpInput, TodoWriteInput, TodoWriteInputTodosItemStatus,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn round_trip<T: Serialize + DeserializeOwned>(fixture: Value) -> Result<T, Box<dyn std::error::Error>> {
    let typed: T = serde_json::from_value(fixture.clone())?;
    assert_eq!(serde_json::to_value(&typed)?, fixture);
    Ok(typed)
}

#[test]
fn string_literal_fields_decode_known_values_and_keep_unlisted_ones() -> TestResult {
    let known: AgentInput = round_trip(json!({
        "description": "Search",
        "prompt": "find it",
        "model": "sonnet",
        "effort": "xhigh",
        "run_in_background": false
    }))?;
    assert_eq!(known.model, Some(AgentInputModel::Sonnet));
    assert_eq!(known.effort, Some(AgentInputEffort::Xhigh));

    let future: AgentInput = round_trip(json!({"description": "d", "prompt": "p", "model": "claude-next"}))?;
    assert_eq!(future.model, Some(AgentInputModel::Unknown("claude-next".to_owned())));
    Ok(())
}

#[test]
fn omitted_optional_fields_stay_omitted() -> TestResult {
    let minimal: AgentInput = round_trip(json!({"description": "d", "prompt": "p"}))?;
    assert_eq!(minimal.model, None);
    assert_eq!(minimal.isolation, None);
    Ok(())
}

#[test]
fn quoted_property_names_and_numbers_round_trip_exactly() -> TestResult {
    let grep: GrepInput = round_trip(json!({
        "pattern": "needle",
        "output_mode": "content",
        "-B": 2,
        "-A": 2.5,
        "-n": true
    }))?;
    assert_eq!(grep.output_mode, Some(GrepInputOutputMode::Content));
    assert_eq!(grep.dash_b.as_ref().map(ToString::to_string), Some("2".to_owned()));
    assert_eq!(grep.dash_a.as_ref().map(ToString::to_string), Some("2.5".to_owned()));
    Ok(())
}

#[test]
fn bounded_tuple_arrays_decode_as_vectors() -> TestResult {
    for count in 1..=4 {
        let questions: Vec<Value> = (0..count)
            .map(|index| {
                json!({
                    "question": format!("Question {index}?"),
                    "header": "Pick",
                    "options": [
                        {"label": "A", "description": "first"},
                        {"label": "B", "description": "second"}
                    ],
                    "multiSelect": index % 2 == 0
                })
            })
            .collect();
        let input: AskUserQuestionInput = round_trip(json!({
            "questions": questions,
            "answers": {"Question 0?": "A"},
            "annotations": {"Question 0?": {"notes": "n"}},
            "metadata": {"source": "test"}
        }))?;
        assert_eq!(input.questions.len(), count);
    }
    Ok(())
}

#[test]
fn open_objects_keep_every_key() -> TestResult {
    let mcp: McpInput = round_trip(json!({"anything": [1, {"nested": null}], "other": "x"}))?;
    assert_eq!(mcp.len(), 2);
    Ok(())
}

#[test]
fn empty_objects_round_trip() -> TestResult {
    round_trip::<EnterPlanModeInput>(json!({}))?;
    Ok(())
}

fn reencodes_identically<T: Serialize + DeserializeOwned>(text: &str) -> TestResult {
    let typed: T = serde_json::from_str(text)?;
    assert_eq!(serde_json::to_string(&typed)?, text);
    Ok(())
}

#[test]
fn key_order_and_undeclared_keys_re_encode_byte_identically() -> TestResult {
    // Declared keys out of declaration order, with undeclared keys before, between and after.
    reencodes_identically::<AgentInput>(
        r#"{"zeta":{"b":1,"a":[3,2,1]},"prompt":"p","future":null,"description":"d","model":"sonnet","alpha":true}"#,
    )?;
    // Undeclared keys inside a closed nested object.
    reencodes_identically::<TodoWriteInput>(
        r#"{"todos":[{"status":"pending","note":{"y":1,"x":2},"content":"c","activeForm":"a","flag":false}]}"#,
    )?;
    reencodes_identically::<GrepInput>(r#"{"-n":true,"pattern":"needle","-B":2,"-A":2.5,"output_mode":"content"}"#)?;
    reencodes_identically::<AskUserQuestionInput>(
        r#"{"answers":{"z?":"B","a?":"A"},"questions":[{"multiSelect":true,"question":"q?","header":"h","options":[{"label":"A","description":"d","preview":"p"},{"description":"e","label":"B"}]}]}"#,
    )?;
    reencodes_identically::<McpInput>(r#"{"z":1,"a":{"y":null,"b":[2,1]},"m":"x"}"#)?;
    Ok(())
}

#[test]
fn undeclared_keys_are_kept_in_order_beside_the_declared_ones() -> TestResult {
    let input: AgentInput = serde_json::from_str(r#"{"zeta":1,"prompt":"p","alpha":2,"description":"d"}"#)?;
    let keys: Vec<&str> = input.extra.keys().map(String::as_str).collect();
    assert_eq!(keys, ["zeta", "alpha"]);
    Ok(())
}

#[test]
fn a_field_changed_after_decoding_keeps_its_position_and_new_ones_follow() -> TestResult {
    let mut input: AgentInput = serde_json::from_str(r#"{"prompt":"p","description":"d"}"#)?;
    input.prompt = "changed".to_owned();
    input.model = Some(AgentInputModel::Opus);
    assert_eq!(
        serde_json::to_string(&input)?,
        r#"{"prompt":"changed","description":"d","model":"opus"}"#
    );
    Ok(())
}

#[test]
fn nested_enums_in_arrays_round_trip() -> TestResult {
    let todos: TodoWriteInput = round_trip(json!({
        "todos": [
            {"content": "a", "status": "in_progress", "activeForm": "Doing a"},
            {"content": "b", "status": "someday", "activeForm": "Doing b"}
        ]
    }))?;
    assert_eq!(todos.todos[0].status, TodoWriteInputTodosItemStatus::InProgress);
    assert_eq!(
        todos.todos[1].status,
        TodoWriteInputTodosItemStatus::Unknown("someday".to_owned())
    );
    Ok(())
}
