use serde_json::Value;
use trogon_claude_agent_sdk::upstream::{
    tool_inputs::ToolInputSchemas,
    tool_outputs::{AgentOutput, ArtifactOutput, FileReadOutput, McpOutput, ProjectsOutput, ToolOutputSchemas},
};
use trogon_claude_agent_sdk::{
    BuiltinToolName, CancellationToken, Content, ContentBlock, ControlRequestId, FeatureSupport, HookRequest,
    JsonObject, PermissionDecision, ResultMessage, ResultStatus, SessionId, ToolName, ToolPermissionRequest, ToolSet,
    ToolUseId, UserMessage,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const INPUT_FIXTURES: &str = include_str!("fixtures/upstream_tool_inputs.json");
const OUTPUT_FIXTURES: &str = include_str!("fixtures/upstream_tool_outputs.json");
const TRANSCRIPT_FIXTURES: &str = include_str!("fixtures/typed_tool_views.json");

const MCP_TOOL: &str = "mcp__example__search";

/// Each tool name with the `sdk-tools` input and output type it is declared with.
const DECLARED: [(&str, &str, &str); 44] = [
    ("Agent", "AgentInput", "AgentOutput"),
    ("Artifact", "ArtifactInput", "ArtifactOutput"),
    ("AskUserQuestion", "AskUserQuestionInput", "AskUserQuestionOutput"),
    ("Bash", "BashInput", "BashOutput"),
    ("ClaudeDesign", "ClaudeDesignInput", "ClaudeDesignOutput"),
    ("CronCreate", "CronCreateInput", "CronCreateOutput"),
    ("CronDelete", "CronDeleteInput", "CronDeleteOutput"),
    ("CronList", "CronListInput", "CronListOutput"),
    ("Edit", "FileEditInput", "FileEditOutput"),
    ("EnterPlanMode", "EnterPlanModeInput", "EnterPlanModeOutput"),
    ("EnterWorktree", "EnterWorktreeInput", "EnterWorktreeOutput"),
    ("ExitPlanMode", "ExitPlanModeInput", "ExitPlanModeOutput"),
    ("ExitWorktree", "ExitWorktreeInput", "ExitWorktreeOutput"),
    ("Glob", "GlobInput", "GlobOutput"),
    ("Grep", "GrepInput", "GrepOutput"),
    (
        "ListMcpResourcesTool",
        "ListMcpResourcesInput",
        "ListMcpResourcesOutput",
    ),
    ("Monitor", "MonitorInput", "MonitorOutput"),
    ("NotebookEdit", "NotebookEditInput", "NotebookEditOutput"),
    ("OfferChromeSetup", "OfferChromeSetupInput", "OfferChromeSetupOutput"),
    ("Projects", "ProjectsInput", "ProjectsOutput"),
    ("propose_skills", "ProposeSkillsInput", "ProposeSkillsOutput"),
    ("ProposeGoal", "ProposeGoalInput", "ProposeGoalOutput"),
    ("PushNotification", "PushNotificationInput", "PushNotificationOutput"),
    ("Read", "FileReadInput", "FileReadOutput"),
    (
        "ReadMcpResourceDirTool",
        "ReadMcpResourceDirInput",
        "ReadMcpResourceDirOutput",
    ),
    ("ReadMcpResourceTool", "ReadMcpResourceInput", "ReadMcpResourceOutput"),
    ("ReadNotifications", "ReadNotificationsInput", "ReadNotificationsOutput"),
    ("RefreshMcpTools", "RefreshMcpToolsInput", "RefreshMcpToolsOutput"),
    ("RemoteTrigger", "RemoteTriggerInput", "RemoteTriggerOutput"),
    ("ReportFindings", "ReportFindingsInput", "ReportFindingsOutput"),
    ("ScheduleWakeup", "ScheduleWakeupInput", "ScheduleWakeupOutput"),
    ("SendFeedback", "SendFeedbackInput", "SendFeedbackOutput"),
    (
        "ShowOnboardingRolePicker",
        "ShowOnboardingRolePickerInput",
        "ShowOnboardingRolePickerOutput",
    ),
    ("TaskCreate", "TaskCreateInput", "TaskCreateOutput"),
    ("TaskGet", "TaskGetInput", "TaskGetOutput"),
    ("TaskList", "TaskListInput", "TaskListOutput"),
    ("TaskStop", "TaskStopInput", "TaskStopOutput"),
    ("TaskUpdate", "TaskUpdateInput", "TaskUpdateOutput"),
    ("TodoWrite", "TodoWriteInput", "TodoWriteOutput"),
    ("WebFetch", "WebFetchInput", "WebFetchOutput"),
    ("WebSearch", "WebSearchInput", "WebSearchOutput"),
    ("Workflow", "WorkflowInput", "WorkflowOutput"),
    ("Write", "FileWriteInput", "FileWriteOutput"),
    (MCP_TOOL, "McpInput", "McpOutput"),
];

/// The built-in names the pinned CLI lists, in its order.
const CLI_BUILTINS: [&str; 54] = [
    "Agent",
    "Artifact",
    "AskUserQuestion",
    "Bash",
    "ClaudeDesign",
    "CronCreate",
    "CronDelete",
    "CronList",
    "Edit",
    "EndConversation",
    "EnterPlanMode",
    "EnterWorktree",
    "ExitPlanMode",
    "ExitWorktree",
    "Glob",
    "Grep",
    "ListAgents",
    "ListMcpResourcesTool",
    "LSP",
    "Monitor",
    "NotebookEdit",
    "OfferChromeSetup",
    "PowerShell",
    "Projects",
    "propose_skills",
    "ProposeGoal",
    "PushNotification",
    "Read",
    "ReadMcpResourceDirTool",
    "ReadMcpResourceTool",
    "ReadNotifications",
    "RemoteTrigger",
    "ReportFindings",
    "ScheduleWakeup",
    "SendFeedback",
    "SendMessage",
    "SendUserFile",
    "SendUserMessage",
    "ShareOnboardingGuide",
    "ShowOnboardingRolePicker",
    "Skill",
    "SubagentHandback",
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskStop",
    "TaskUpdate",
    "TodoWrite",
    "ToolSearch",
    "WaitForMcpServers",
    "WebFetch",
    "WebSearch",
    "Workflow",
    "Write",
];

fn samples(fixtures: &str, type_name: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut all: serde_json::Map<String, Value> = serde_json::from_str(fixtures)?;
    match all.remove(type_name) {
        Some(Value::Array(samples)) if !samples.is_empty() => Ok(samples),
        _ => Err(format!("fixture {type_name} has no samples").into()),
    }
}

fn is_variant(debug: &str, variant: &str) -> bool {
    debug.starts_with(&format!("{variant}("))
}

fn value(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(text)
}

fn object(text: &str) -> Result<JsonObject, serde_json::Error> {
    serde_json::from_str(text)
}

#[test]
fn builtin_constants_name_every_cli_tool_in_cli_order() {
    let names: Vec<&str> = BuiltinToolName::ALL.iter().map(BuiltinToolName::as_str).collect();
    assert_eq!(names, CLI_BUILTINS);
    assert_eq!(BuiltinToolName::REFRESH_MCP_TOOLS.as_str(), "RefreshMcpTools");
    assert_eq!(BuiltinToolName::BASH, BuiltinToolName::from("Bash"));
    assert_eq!(BuiltinToolName::PROPOSE_SKILLS.as_str(), "propose_skills");
    let only = ToolSet::Only(vec![BuiltinToolName::BASH, BuiltinToolName::READ]);
    assert!(matches!(&only, ToolSet::Only(names) if names.len() == 2));
}

#[test]
fn legacy_aliases_resolve_like_the_cli_and_other_names_pass_through() {
    let expected = [
        ("Task", "Agent"),
        ("KillShell", "TaskStop"),
        ("KillBash", "TaskStop"),
        ("ListPeers", "ListAgents"),
        ("Brief", "SendUserMessage"),
        ("ListMcpResources", "ListMcpResourcesTool"),
        ("ReadMcpResource", "ReadMcpResourceTool"),
        ("ReadMcpResourceDir", "ReadMcpResourceDirTool"),
    ];
    assert_eq!(BuiltinToolName::LEGACY_ALIASES, expected);
    for (alias, target) in expected {
        assert_eq!(BuiltinToolName::canonical_name(alias), target);
        assert!(CLI_BUILTINS.contains(&target), "{target} is not a CLI built-in");
    }
    for name in ["Bash", "Eval", "FutureTool", "mcp__server__tool", ""] {
        assert_eq!(BuiltinToolName::canonical_name(name), name);
    }
}

#[test]
fn every_declared_input_decodes_to_its_tool_type_and_reencodes_unchanged() -> TestResult {
    for (tool, input_type, _) in DECLARED {
        for sample in samples(INPUT_FIXTURES, input_type)? {
            let typed = ToolInputSchemas::for_tool(tool, &sample);
            let debug = format!("{typed:?}");
            assert!(is_variant(&debug, input_type), "{tool} input decoded as {debug}");
            assert_eq!(serde_json::to_value(&typed)?, sample, "{tool} input changed");
        }
    }
    Ok(())
}

#[test]
fn every_declared_output_decodes_to_its_tool_type_and_reencodes_unchanged() -> TestResult {
    let unions = [
        "AgentOutput",
        "ArtifactOutput",
        "ProjectsOutput",
        "FileReadOutput",
        "McpOutput",
    ];
    for (tool, _, output_type) in DECLARED {
        let mut typed_hits = 0;
        for sample in samples(OUTPUT_FIXTURES, output_type)? {
            let typed = ToolOutputSchemas::for_tool(tool, &sample);
            let debug = format!("{typed:?}");
            let union_miss = unions.contains(&output_type) && is_variant(&debug, "Unknown");
            assert!(
                is_variant(&debug, output_type) || union_miss,
                "{tool} output decoded as {debug}"
            );
            typed_hits += usize::from(is_variant(&debug, output_type));
            assert_eq!(serde_json::to_value(&typed)?, sample, "{tool} output changed");
        }
        assert!(typed_hits > 0, "no {output_type} sample decoded as {output_type}");
    }
    Ok(())
}

/// Every sample of `type_name` viewed through `tool` matches what `pick`, the generated union's own matcher, picks.
fn matches_generated_union(
    tool: &str,
    type_name: &str,
    pick: impl Fn(&Value) -> Option<ToolOutputSchemas>,
) -> TestResult {
    for sample in samples(OUTPUT_FIXTURES, type_name)? {
        let expected = pick(&sample).unwrap_or_else(|| ToolOutputSchemas::Unknown(sample.clone()));
        assert_eq!(
            ToolOutputSchemas::for_tool(tool, &sample),
            expected,
            "{tool} picked another member"
        );
    }
    Ok(())
}

#[test]
fn union_outputs_pick_the_member_the_generated_union_picks() -> TestResult {
    matches_generated_union("Agent", "AgentOutput", |sample| {
        AgentOutput::best_match(sample).map(|(_, inner)| ToolOutputSchemas::AgentOutput(inner))
    })?;
    matches_generated_union("Artifact", "ArtifactOutput", |sample| {
        ArtifactOutput::best_match(sample).map(|(_, inner)| ToolOutputSchemas::ArtifactOutput(inner))
    })?;
    matches_generated_union("Projects", "ProjectsOutput", |sample| {
        ProjectsOutput::best_match(sample).map(|(_, inner)| ToolOutputSchemas::ProjectsOutput(inner))
    })?;
    matches_generated_union("Read", "FileReadOutput", |sample| {
        FileReadOutput::best_match(sample).map(|(_, inner)| ToolOutputSchemas::FileReadOutput(inner))
    })?;
    matches_generated_union(MCP_TOOL, "McpOutput", |sample| {
        McpOutput::best_match(sample).map(|(_, inner)| ToolOutputSchemas::McpOutput(inner))
    })
}

#[test]
fn legacy_aliases_decode_to_the_type_of_the_tool_they_resolve_to() -> TestResult {
    let resolved = [
        ("Task", "AgentInput", "AgentOutput"),
        ("KillShell", "TaskStopInput", "TaskStopOutput"),
        ("KillBash", "TaskStopInput", "TaskStopOutput"),
        ("ListMcpResources", "ListMcpResourcesInput", "ListMcpResourcesOutput"),
        ("ReadMcpResource", "ReadMcpResourceInput", "ReadMcpResourceOutput"),
        (
            "ReadMcpResourceDir",
            "ReadMcpResourceDirInput",
            "ReadMcpResourceDirOutput",
        ),
    ];
    for (alias, input_type, output_type) in resolved {
        let input = samples(INPUT_FIXTURES, input_type)?.remove(0);
        let output = samples(OUTPUT_FIXTURES, output_type)?
            .into_iter()
            .find(|sample| {
                is_variant(
                    &format!("{:?}", ToolOutputSchemas::for_tool(alias, sample)),
                    output_type,
                )
            })
            .ok_or("no output sample decoded")?;
        assert!(is_variant(
            &format!("{:?}", ToolInputSchemas::for_tool(alias, &input)),
            input_type
        ));
        assert_eq!(
            serde_json::to_value(ToolOutputSchemas::for_tool(alias, &output))?,
            output
        );
    }
    let send_message = value(r#"{"message":"text"}"#)?;
    for alias in ["ListPeers", "Brief"] {
        assert_eq!(
            ToolInputSchemas::for_tool(alias, &send_message),
            ToolInputSchemas::Unknown(send_message.clone())
        );
        assert_eq!(
            ToolOutputSchemas::for_tool(alias, &send_message),
            ToolOutputSchemas::Unknown(send_message.clone())
        );
    }
    Ok(())
}

#[test]
fn values_the_named_type_does_not_reproduce_stay_unknown_and_unchanged() -> TestResult {
    let numeric_command = value(r#"{"command":3}"#)?;
    assert_eq!(
        ToolInputSchemas::for_tool("Bash", &numeric_command),
        ToolInputSchemas::Unknown(numeric_command.clone())
    );
    assert_eq!(
        ToolOutputSchemas::for_tool("Bash", &numeric_command),
        ToolOutputSchemas::Unknown(numeric_command.clone())
    );
    let number = value("3")?;
    for tool in ["Read", "Agent", "Artifact", "Projects", MCP_TOOL] {
        assert_eq!(
            ToolOutputSchemas::for_tool(tool, &number),
            ToolOutputSchemas::Unknown(number.clone())
        );
    }
    let text = value(r#""text""#)?;
    assert_eq!(
        ToolInputSchemas::for_tool(MCP_TOOL, &text),
        ToolInputSchemas::Unknown(text.clone())
    );
    let mcp_object = value(r#"{"query":"text","limit":3}"#)?;
    assert!(is_variant(
        &format!("{:?}", ToolInputSchemas::for_tool(MCP_TOOL, &mcp_object)),
        "McpInput"
    ));
    let undeclared = value(r#"{"skill":"text"}"#)?;
    for tool in ["Skill", "ToolSearch", "FutureTool", "mcp_", ""] {
        assert_eq!(
            ToolInputSchemas::for_tool(tool, &undeclared),
            ToolInputSchemas::Unknown(undeclared.clone())
        );
        assert_eq!(
            ToolOutputSchemas::for_tool(tool, &undeclared),
            ToolOutputSchemas::Unknown(undeclared.clone())
        );
    }
    Ok(())
}

#[test]
fn recorded_cli_tool_calls_decode_to_the_declared_types() -> TestResult {
    let fixture: Value = serde_json::from_str(TRANSCRIPT_FIXTURES)?;
    let cases = fixture
        .get("cases")
        .and_then(Value::as_array)
        .ok_or("fixture has no cases")?;
    assert!(!cases.is_empty());
    for case in cases {
        let tool = case
            .get("tool_name")
            .and_then(Value::as_str)
            .ok_or("case has no tool_name")?;
        let input = case.get("input").ok_or("case has no input")?;
        let result = case.get("tool_use_result").ok_or("case has no tool_use_result")?;
        let input_type = case.get("input_type").and_then(Value::as_str).unwrap_or("Unknown");
        let output_type = case.get("output_type").and_then(Value::as_str).unwrap_or("Unknown");
        let typed_input = ToolInputSchemas::for_tool(tool, input);
        let typed_output = ToolOutputSchemas::for_tool(tool, result);
        assert!(
            is_variant(&format!("{typed_input:?}"), input_type),
            "{tool} input: {typed_input:?}"
        );
        assert!(
            is_variant(&format!("{typed_output:?}"), output_type),
            "{tool} output: {typed_output:?}"
        );
        assert_eq!(&serde_json::to_value(&typed_input)?, input);
        assert_eq!(&serde_json::to_value(&typed_output)?, result);
    }
    Ok(())
}

#[test]
fn tool_use_blocks_expose_a_typed_input_and_keep_the_raw_one() -> TestResult {
    let input = object(r#"{"command":"ls","description":"text"}"#)?;
    let block = ContentBlock::ToolUse {
        id: ToolUseId::from("toolu_1"),
        name: ToolName::from("Bash"),
        input: input.clone(),
    };
    let typed = block.typed_input().ok_or("tool_use block has no typed input")?;
    assert!(is_variant(&format!("{typed:?}"), "BashInput"));
    assert_eq!(serde_json::to_value(&typed)?, Value::Object(input.clone()));
    assert!(matches!(&block, ContentBlock::ToolUse { input: kept, .. } if *kept == input));
    let text = ContentBlock::Text { text: "text".into() };
    assert_eq!(text.typed_input(), None);
    Ok(())
}

#[test]
fn user_messages_expose_a_typed_tool_use_result_and_keep_the_raw_one() -> TestResult {
    let result = value(r#"{"stdout":"text","stderr":"","interrupted":false}"#)?;
    let message = UserMessage {
        content: Content::Text("text".into()),
        parent_tool_use_id: None,
        session_id: None,
        uuid: None,
        origin: None,
        tool_use_result: Some(result.clone()),
        additional_fields: JsonObject::new(),
        message_fields: JsonObject::new(),
        raw: JsonObject::new(),
    };
    let typed = message
        .typed_tool_use_result("Bash")
        .ok_or("no typed tool_use_result")?;
    assert!(is_variant(&format!("{typed:?}"), "BashOutput"));
    assert_eq!(message.tool_use_result, Some(result.clone()));
    assert_eq!(
        message.typed_tool_use_result("FutureTool"),
        Some(ToolOutputSchemas::Unknown(result))
    );
    let without = UserMessage {
        tool_use_result: None,
        ..message
    };
    assert_eq!(without.typed_tool_use_result("Bash"), None);
    Ok(())
}

fn permission_request(tool_name: &str, input: JsonObject) -> ToolPermissionRequest {
    ToolPermissionRequest {
        tool_name: ToolName::from(tool_name),
        input,
        tool_use_id: None,
        blocked_path: None,
        decision_reason: None,
        request_id: ControlRequestId::from("request-1"),
        signal: CancellationToken::new(),
        suggestions: Vec::new(),
        agent_id: None,
        mcp_server: None,
        title: None,
        display_name: None,
        description: None,
        default_to_no: FeatureSupport::NotReported,
        suppress_always_allow_rule: FeatureSupport::NotReported,
        requires_user_interaction: FeatureSupport::NotReported,
        decision_reason_type: None,
        decision_reason_code: None,
        server_policy_ask_only: FeatureSupport::NotReported,
        plugin: None,
        server_prompt: None,
        computer_folder: None,
        matched_ask_rule: None,
        additional_fields: JsonObject::new(),
    }
}

#[test]
fn permission_requests_expose_a_typed_input_that_allows_with_an_updated_input() -> TestResult {
    let input = object(r#"{"file_path":"/tmp/example.txt","offset":3}"#)?;
    let request = permission_request("Read", input.clone());
    let typed = request.typed_input();
    assert!(is_variant(&format!("{typed:?}"), "FileReadInput"));
    assert_eq!(request.input, input);
    let decision = PermissionDecision::allow_with_input(&typed).ok_or("object input was refused")?;
    assert!(matches!(
        &decision,
        PermissionDecision::Allow {
            updated_input: Some(updated),
            updated_permissions: None,
            decision_classification: None,
        } if *updated == input
    ));
    let unknown = permission_request("FutureTool", input.clone()).typed_input();
    assert_eq!(unknown, ToolInputSchemas::Unknown(Value::Object(input)));
    let not_an_object = ToolInputSchemas::Unknown(value(r#""text""#)?);
    assert!(PermissionDecision::allow_with_input(&not_an_object).is_none());
    Ok(())
}

fn hook_request(input: &str) -> Result<HookRequest, serde_json::Error> {
    Ok(HookRequest {
        input: object(input)?,
        tool_use_id: None,
        signal: CancellationToken::new(),
    })
}

#[test]
fn hook_requests_expose_typed_tool_inputs_and_responses() -> TestResult {
    let request = hook_request(
        r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"ls"},"tool_response":{"stdout":"text","stderr":"","interrupted":false}}"#,
    )?;
    let input = request.typed_tool_input().ok_or("no typed tool_input")?;
    let response = request.typed_tool_response().ok_or("no typed tool_response")?;
    assert!(is_variant(&format!("{input:?}"), "BashInput"));
    assert!(is_variant(&format!("{response:?}"), "BashOutput"));
    assert_eq!(
        Some(&serde_json::to_value(&response)?),
        request.input.get("tool_response")
    );
    let pre = hook_request(r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"}}"#)?;
    assert!(pre.typed_tool_input().is_some());
    assert_eq!(pre.typed_tool_response(), None);
    let numeric_name = hook_request(r#"{"tool_name":3,"tool_input":{},"tool_response":{}}"#)?;
    assert_eq!(numeric_name.typed_tool_input(), None);
    assert_eq!(numeric_name.typed_tool_response(), None);
    let no_tool = hook_request(r#"{"hook_event_name":"Stop"}"#)?;
    assert_eq!(no_tool.typed_tool_input(), None);
    assert_eq!(no_tool.typed_tool_response(), None);
    let no_input = hook_request(r#"{"tool_name":"Bash"}"#)?;
    assert_eq!(no_input.typed_tool_input(), None);
    Ok(())
}

fn result_message(permission_denials: Option<Vec<Value>>) -> ResultMessage {
    ResultMessage {
        subtype: "success".into(),
        status: ResultStatus::Success,
        duration_ms: 3,
        duration_api_ms: 3,
        num_turns: 1,
        session_id: SessionId::from("00000000-0000-4000-8000-000000000001"),
        total_cost_usd: None,
        result: None,
        structured_output: None,
        usage: None,
        errors: None,
        uuid: None,
        stop_reason: None,
        terminal_reason: None,
        origin: None,
        model_usage: None,
        permission_denials,
        deferred_tool_use: None,
        api_error_status: None,
        result_index: None,
        queued_turn_count: None,
        additional_fields: JsonObject::new(),
        raw: JsonObject::new(),
    }
}

#[test]
fn permission_denials_keep_every_entry_with_raw_and_typed_access() -> TestResult {
    let valid = value(r#"{"tool_name":"Bash","tool_use_id":"toolu_1","tool_input":{"command":"ls"}}"#)?;
    let malformed = value(r#"{"tool_name":3,"tool_use_id":[],"tool_input":"text"}"#)?;
    let missing_input = value(r#"{"tool_name":"Bash"}"#)?;
    let message = result_message(Some(vec![
        valid.clone(),
        malformed.clone(),
        missing_input.clone(),
        Value::Null,
    ]));
    let denials = message.typed_permission_denials().ok_or("no permission denials")?;
    assert_eq!(denials.len(), 4);
    let raws: Vec<&Value> = denials.iter().map(|denial| &denial.raw).collect();
    assert_eq!(raws, [&valid, &malformed, &missing_input, &Value::Null]);

    let first = &denials[0];
    assert_eq!(first.tool_name(), Some("Bash"));
    assert_eq!(first.tool_use_id(), Some("toolu_1"));
    assert_eq!(first.tool_input(), Some(&object(r#"{"command":"ls"}"#)?));
    let typed = first.typed_input().ok_or("no typed input")?;
    assert!(is_variant(&format!("{typed:?}"), "BashInput"));
    let sdk = first.to_sdk_permission_denial().ok_or("not an SDKPermissionDenial")?;
    assert_eq!(sdk.tool_name, "Bash");
    assert_eq!(sdk.tool_use_id, "toolu_1");

    let second = &denials[1];
    assert_eq!(second.tool_name(), None);
    assert_eq!(second.tool_use_id(), None);
    assert_eq!(second.tool_input(), None);
    assert_eq!(second.typed_input(), None);
    assert_eq!(second.to_sdk_permission_denial(), None);

    assert_eq!(denials[2].tool_name(), Some("Bash"));
    assert_eq!(denials[2].typed_input(), None);
    assert_eq!(denials[3].tool_name(), None);
    assert_eq!(denials[3].clone(), denials[3]);

    assert_eq!(result_message(None).typed_permission_denials(), None);
    Ok(())
}
