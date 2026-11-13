use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use trogon_claude_agent_sdk::upstream::tool_inputs::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FIXTURES: &str = include_str!("fixtures/upstream_tool_inputs.json");

fn fixtures() -> Result<serde_json::Map<String, Value>, serde_json::Error> {
    serde_json::from_str(FIXTURES)
}

fn round_trips<T: Serialize + DeserializeOwned>(name: &str, samples: &[Value]) -> TestResult {
    for sample in samples {
        let typed: T = serde_json::from_value(sample.clone())?;
        assert_eq!(&serde_json::to_value(&typed)?, sample, "{name} did not round-trip");
        let text = serde_json::to_string(sample)?;
        let reparsed: T = serde_json::from_str(&text)?;
        assert_eq!(
            serde_json::to_value(&reparsed)?,
            *sample,
            "{name} did not round-trip as text"
        );
    }
    Ok(())
}

/// The same value with the keys of every object in reverse order.
fn reversed(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .rev()
                .map(|(key, inner)| (key.clone(), reversed(inner)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(reversed).collect()),
        other => other.clone(),
    }
}

/// The same object with an undeclared, nested key placed first.
fn with_leading_unknown_key(value: &Value) -> Option<Value> {
    let map = value.as_object()?;
    let mut out = serde_json::Map::new();
    out.insert(
        "zz_future".to_owned(),
        serde_json::from_str(r#"{"b":1,"a":[2,{"d":null,"c":3}]}"#).ok()?,
    );
    out.extend(map.clone());
    Some(Value::Object(out))
}

fn reencodes_identically<T: Serialize + DeserializeOwned>(name: &str, text: &str) -> TestResult {
    let typed: T = serde_json::from_str(text)?;
    assert_eq!(
        serde_json::to_string(&typed)?,
        text,
        "{name} did not re-encode byte-identically"
    );
    Ok(())
}

macro_rules! every_input {
    ($($name:ident),+ $(,)?) => {
        const COVERED: &[&str] = &[$(stringify!($name)),+];

        fn check_bytes(fixtures: &serde_json::Map<String, Value>) -> TestResult {
            $(
                let samples = fixtures
                    .get(stringify!($name))
                    .and_then(Value::as_array)
                    .ok_or(concat!("missing fixture ", stringify!($name)))?;
                for sample in samples {
                    reencodes_identically::<$name>(stringify!($name), &serde_json::to_string(sample)?)?;
                    reencodes_identically::<$name>(stringify!($name), &serde_json::to_string(&reversed(sample))?)?;
                }
            )+
            Ok(())
        }

        fn check_all(fixtures: &serde_json::Map<String, Value>) -> TestResult {
            $(
                let samples = fixtures
                    .get(stringify!($name))
                    .and_then(Value::as_array)
                    .ok_or(concat!("missing fixture ", stringify!($name)))?;
                round_trips::<$name>(stringify!($name), samples)?;
            )+
            Ok(())
        }
    };
}

every_input!(
    AgentInput,
    BashInput,
    ExitPlanModeInput,
    FileEditInput,
    FileReadInput,
    FileWriteInput,
    GlobInput,
    GrepInput,
    TaskStopInput,
    ListMcpResourcesInput,
    RefreshMcpToolsInput,
    McpInput,
    NotebookEditInput,
    ReadMcpResourceDirInput,
    ReadMcpResourceInput,
    ReportFindingsInput,
    TodoWriteInput,
    WebFetchInput,
    WebSearchInput,
    AskUserQuestionInput,
    SendFeedbackInput,
    ClaudeDesignInput,
    ProjectsInput,
    EnterPlanModeInput,
    TaskCreateInput,
    TaskGetInput,
    TaskUpdateInput,
    TaskListInput,
    WorkflowInput,
    CronCreateInput,
    CronDeleteInput,
    CronListInput,
    ScheduleWakeupInput,
    RemoteTriggerInput,
    ShowOnboardingRolePickerInput,
    OfferChromeSetupInput,
    ReadNotificationsInput,
    MonitorInput,
    ProposeSkillsInput,
    ProposeGoalInput,
    ArtifactInput,
    PushNotificationInput,
    EnterWorktreeInput,
    ExitWorktreeInput,
);

#[test]
fn fixtures_cover_exactly_the_declared_inputs() -> TestResult {
    let fixtures = fixtures()?;
    let mut names: Vec<&str> = fixtures.keys().map(String::as_str).collect();
    names.sort_unstable();
    let mut covered = COVERED.to_vec();
    covered.sort_unstable();
    assert_eq!(names, covered);
    Ok(())
}

#[test]
fn every_built_in_tool_input_round_trips_its_upstream_shaped_fixture() -> TestResult {
    check_all(&fixtures()?)
}

#[test]
fn the_input_union_round_trips_every_fixture_without_falling_back() -> TestResult {
    for (name, samples) in fixtures()? {
        for sample in samples.as_array().ok_or("fixture is not an array")? {
            let typed: ToolInputSchemas = serde_json::from_value(sample.clone())?;
            assert!(
                !matches!(typed, ToolInputSchemas::Unknown(_)),
                "{name} fell back to raw JSON"
            );
            assert_eq!(
                &serde_json::to_value(&typed)?,
                sample,
                "{name} did not round-trip through the union"
            );
        }
    }
    Ok(())
}

#[test]
fn every_fixture_re_encodes_byte_identically_in_original_and_reversed_key_order() -> TestResult {
    check_bytes(&fixtures()?)
}

#[test]
fn the_input_union_re_encodes_every_fixture_byte_identically_with_unknown_keys() -> TestResult {
    for (name, samples) in fixtures()? {
        for sample in samples.as_array().ok_or("fixture is not an array")? {
            let mut shapes = vec![sample.clone(), reversed(sample)];
            shapes.extend(with_leading_unknown_key(sample));
            shapes.extend(with_leading_unknown_key(&reversed(sample)));
            for shape in shapes {
                reencodes_identically::<ToolInputSchemas>(&name, &serde_json::to_string(&shape)?)?;
            }
        }
    }
    Ok(())
}

#[test]
fn the_input_union_has_exactly_the_members_upstream_declares() -> TestResult {
    // Upstream's `ToolInputSchemas` also lists `ToolOutputSchemas`; it stays a member.
    let output: Value = serde_json::from_str(r#"{"stdout": "x", "stderr": "", "interrupted": false}"#)?;
    let typed: ToolInputSchemas = serde_json::from_value(output.clone())?;
    assert!(matches!(typed, ToolInputSchemas::ToolOutputSchemas(_)), "{typed:?}");
    assert_eq!(serde_json::to_value(&typed)?, output);
    Ok(())
}

#[test]
fn the_input_union_keeps_unrecognised_shapes_in_its_own_unknown_variant() -> TestResult {
    let sample: Value = serde_json::from_str(r#"[1, "two", {"three": null}]"#)?;
    let typed: ToolInputSchemas = serde_json::from_value(sample.clone())?;
    assert_eq!(typed, ToolInputSchemas::Unknown(sample.clone()));
    assert_eq!(serde_json::to_value(&typed)?, sample);
    Ok(())
}

#[test]
fn a_known_input_with_an_undeclared_key_stays_that_input_inside_the_union() -> TestResult {
    let text = r#"{"future_option":{"b":1,"a":2},"command":"ls"}"#;
    let typed: ToolInputSchemas = serde_json::from_str(text)?;
    let ToolInputSchemas::BashInput(bash) = &typed else {
        return Err(format!("not decoded as BashInput: {typed:?}").into());
    };
    assert_eq!(
        bash.extra.keys().map(String::as_str).collect::<Vec<_>>(),
        ["future_option"]
    );
    assert_eq!(serde_json::to_string(&typed)?, text);
    Ok(())
}
