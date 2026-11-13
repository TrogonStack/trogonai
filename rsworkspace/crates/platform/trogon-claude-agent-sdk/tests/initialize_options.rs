#![cfg(all(unix, feature = "test-support"))]
#![allow(clippy::expect_used, clippy::unwrap_used)]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Pins the initialize request, and the Python system prompt flags, against output captured from the real
//! `sdk.mjs` 0.3.323 and `claude-agent-sdk` 0.2.181 for the same options.
//! The fixture holds the request bytes each SDK wrote. TypeScript writes compact JSON; Python writes its own
//! separators, so its requests are stored re-serialized from the parsed value with the key order kept.

use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    AgentDefinition, ArgvStyle, CallbackError, ClaudeAgentOptions, ClaudeSdkClient, CustomPrompt, DialogKind,
    PresetPrompt, Prompt, Skills, SystemPrompt, UserDialogRequest, VerbatimPrompts, WorkspaceTrust,
    adapters::{NativeFileSystem, RuntimeAdapters},
    test_support::FixedEnvironment,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;

use scripted_process::{ScriptedProcessHarness, ScriptedProcessSpawner};

const FIXTURE: &str = include_str!("fixtures/initialize_options.json");

struct Capture {
    arguments: Vec<String>,
    initialize: Value,
    user_line: Option<String>,
}

async fn serve(process: ScriptedProcessHarness, expect_user: bool) -> Result<(String, Option<String>), std::io::Error> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let stderr = process.stderr;
    let (mut initialize, mut user) = (None, None);
    while let Some(line) = lines.next_line().await? {
        let value: Value = serde_json::from_str(&line)?;
        if value["request"]["subtype"] == "initialize" {
            let response = serde_json::json!({"type":"control_response","response":{
                "subtype":"success","request_id":value["request_id"],
                "response":{"hooks_applied":true,"hook_on_failure_supported":true,"claude_code_version":"2.1.319"}}});
            let mut frame = serde_json::to_vec(&response)?;
            frame.push(b'\n');
            stdout.write_all(&frame).await?;
            initialize = Some(line);
        } else if expect_user && value["type"] == "user" {
            user = Some(line);
        }
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok((
        initialize.ok_or_else(|| std::io::Error::other("no initialize request"))?,
        user,
    ))
}

async fn run(options: ClaudeAgentOptions, send_prompt: bool) -> Result<Capture, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let server = tokio::spawn(serve(harness, send_prompt));
    let client = ClaudeSdkClient::connect_with_adapters(
        options,
        RuntimeAdapters {
            environment: FixedEnvironment {
                directory: PathBuf::from("unused-directory"),
                variables: BTreeMap::new(),
            },
            process,
            filesystem: NativeFileSystem,
        },
    )
    .await?;
    if send_prompt {
        client.send_prompt(Prompt::from("hi @x")).await?;
    }
    client.disconnect().await?;
    let (initialize, user_line) = server.await??;
    let command = observer.commands()?.remove(0);
    Ok(Capture {
        arguments: command
            .arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect(),
        initialize: serde_json::from_str::<Value>(&initialize)?["request"].clone(),
        user_line,
    })
}

fn options(style: ArgvStyle) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        argv_style: style,
        ..ClaudeAgentOptions::default()
    }
}

fn preset(append: Option<&str>, exclude: Option<bool>, snapshot: Option<bool>) -> SystemPrompt {
    SystemPrompt::Preset(PresetPrompt {
        append: append.map(str::to_owned),
        exclude_dynamic_sections: exclude,
        snapshot,
    })
}

fn custom(prompt: &[&str], snapshot: Option<bool>) -> SystemPrompt {
    SystemPrompt::CustomWith(CustomPrompt {
        prompt: prompt.iter().map(|text| (*text).to_owned()).collect(),
        snapshot,
    })
}

fn trust(accepted: Option<bool>, directory: Option<&str>) -> Option<WorkspaceTrust> {
    Some(WorkspaceTrust {
        accepted,
        directory: directory.map(str::to_owned),
    })
}

fn misc_typescript(options: &mut ClaudeAgentOptions, flag: bool) {
    options.prompt_suggestions = Some(flag);
    options.agent_progress_summaries = Some(flag);
    options.forward_subagent_text = Some(flag);
    options.per_task_stop_affordance = Some(flag);
    options.rapid_followup_preempt = Some(flag);
    options.forward_nested_subagent_tool_calls = Some(flag);
}

/// The options the capture harness gave each SDK, for one case.
fn case(name: &str, style: ArgvStyle) -> ClaudeAgentOptions {
    let mut o = options(style);
    let python = style == ArgvStyle::Python;
    match name {
        "sp_string" => o.system_prompt = SystemPrompt::Custom("be brief".into()),
        "sp_array" => o.system_prompt = SystemPrompt::Parts(vec!["a".into(), "b".into()]),
        "sp_empty_string" => o.system_prompt = SystemPrompt::Custom(String::new()),
        "sp_custom" => o.system_prompt = custom(&["p"], Some(true)),
        "sp_custom_nosnap" => o.system_prompt = custom(&["p"], None),
        "sp_custom_array" => o.system_prompt = custom(&["x", "y"], Some(false)),
        "sp_preset" => o.system_prompt = SystemPrompt::ClaudeCode,
        "sp_preset_append" => o.system_prompt = preset(Some("more"), None, None),
        "sp_preset_append_empty" => o.system_prompt = preset(Some(""), None, None),
        "sp_preset_all" => o.system_prompt = preset(Some("more"), Some(true), Some(false)),
        "sp_preset_eds_false" => o.system_prompt = preset(None, Some(false), None),
        "sp_preset_snapshot_true" => o.system_prompt = preset(None, None, Some(true)),
        "sp_file" => o.system_prompt = SystemPrompt::File("prompt.txt".into()),
        "misc_all" if python => o.forward_subagent_text = Some(true),
        "misc_all" => {
            o.plan_mode_instructions = Some("plan it".into());
            o.append_subagent_system_prompt = Some("sub".into());
            o.tool_aliases = Some(vec![
                ("Bash".into(), "mcp__w__bash".into()),
                ("Read".into(), "mcp__w__read".into()),
            ]);
            o.code_running_mcp_tools = Some(vec!["mcp__a__run".into()]);
            o.title = Some("My title".into());
            o.web_search_isolation_exempt_mcp_servers = Some(vec!["srv1".into(), "srv2".into()]);
            misc_typescript(&mut o, true);
        }
        "misc_false" if python => o.forward_subagent_text = Some(false),
        "misc_false" => misc_typescript(&mut o, false),
        "misc_empty" => {
            o.title = Some(String::new());
            o.tool_aliases = Some(Vec::new());
            o.code_running_mcp_tools = Some(Vec::new());
            o.web_search_isolation_exempt_mcp_servers = Some(Vec::new());
            o.plan_mode_instructions = Some(String::new());
            o.append_subagent_system_prompt = Some(String::new());
        }
        "trust_abs" => o.workspace_trust = trust(Some(true), Some("/work/space")),
        "trust_win_drive" => o.workspace_trust = trust(Some(true), Some("C:\\work")),
        "trust_win_unc" => o.workspace_trust = trust(Some(true), Some("\\\\srv\\share")),
        "trust_win_rooted" => o.workspace_trust = trust(Some(true), Some("\\work")),
        "trust_win_drive_rel" => o.workspace_trust = trust(Some(true), Some("C:work")),
        "trust_no_dir" => o.workspace_trust = trust(Some(true), None),
        "trust_relative" => o.workspace_trust = trust(Some(true), Some("rel/dir")),
        "trust_empty_dir" => o.workspace_trust = trust(Some(true), Some("")),
        "trust_declined" => o.workspace_trust = trust(Some(false), None),
        "trust_declined_dir" => o.workspace_trust = trust(Some(false), Some("rel")),
        "trust_empty" => o.workspace_trust = trust(None, None),
        "with_neighbours" => {
            o.system_prompt = preset(Some("a"), Some(true), Some(true));
            o.agents
                .insert("rev".into(), AgentDefinition::new("d", Prompt::from("p")));
            o.skills = Some(Skills::Named(vec!["s1".into()]));
            o.forward_subagent_text = Some(true);
            if !python {
                o.title = Some("t".into());
                o.supported_dialog_kinds = vec![DialogKind::from("refusal_fallback_prompt")];
                o.user_dialog_handler = Some(Arc::new(|_: UserDialogRequest| async { Err(CallbackError::Cancelled) }));
                o.per_task_stop_affordance = Some(true);
                o.tool_aliases = Some(vec![("A".into(), "B".into())]);
            }
        }
        "verbatim" => o.verbatim_prompts = VerbatimPrompts::Enabled,
        "verbatim_off" => o.verbatim_prompts = VerbatimPrompts::Disabled,
        _ => {}
    }
    o
}

fn compact(value: &Value) -> String {
    serde_json::to_string(value).expect("a parsed value serializes")
}

fn prompt_flags(arguments: &[String]) -> Vec<String> {
    let mut flags = Vec::new();
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        if matches!(
            argument.as_str(),
            "--system-prompt" | "--system-prompt-file" | "--append-system-prompt"
        ) {
            flags.push(argument.clone());
            flags.extend(remaining.next().cloned());
        }
    }
    flags
}

#[tokio::test]
async fn every_case_matches_the_captured_initialize_bytes_in_both_dialects() -> Result<(), Box<dyn std::error::Error>> {
    let fixture: BTreeMap<String, BTreeMap<String, Value>> = serde_json::from_str(FIXTURE)?;
    assert!(fixture.len() >= 30);
    for (name, golden) in &fixture {
        if let Some(expected) = golden.get("ts_request") {
            let verbatim = name.starts_with("verbatim");
            let capture = run(case(name, ArgvStyle::TypeScript), verbatim).await?;
            assert_eq!(
                compact(&capture.initialize),
                expected.as_str().unwrap_or_default(),
                "ts {name}"
            );
            if let Some(expected) = golden.get("ts_user_client_composed") {
                let line = capture.user_line.as_deref().unwrap_or_default();
                assert_eq!(
                    line.ends_with(",\"client_composed\":true}"),
                    expected.as_bool().unwrap_or(false),
                    "ts {name} user"
                );
            }
        }
        if let Some(expected) = golden.get("py_request") {
            let verbatim = name.starts_with("verbatim");
            let capture = run(case(name, ArgvStyle::Python), verbatim).await?;
            assert_eq!(
                compact(&capture.initialize),
                expected.as_str().unwrap_or_default(),
                "py {name}"
            );
            let flags: Vec<String> = serde_json::from_value(golden["py_prompt_argv"].clone())?;
            assert_eq!(prompt_flags(&capture.arguments), flags, "py {name} argv");
            if let Some(expected) = golden.get("py_user_client_composed") {
                // Python writes the user message with `json.dumps` and its default separators.
                let line = capture.user_line.as_deref().unwrap_or_default();
                assert_eq!(
                    line.ends_with(", \"client_composed\": true}"),
                    expected.as_bool().unwrap_or(false),
                    "py {name} user"
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn typescript_refuses_a_prompt_file_and_python_refuses_several_prompt_texts() {
    let file = ClaudeSdkClient::connect(case("sp_file", ArgvStyle::TypeScript)).await;
    assert!(file.is_err());
    let parts = ClaudeSdkClient::connect(case("sp_array", ArgvStyle::Python)).await;
    assert!(parts.is_err());
}

#[test]
fn manifest_capture_defaults_to_on_unless_the_disable_variable_is_truthy() {
    let options = trogon_claude_agent_sdk::ClaudeAgentOptions::default();
    assert!(options.captures_sdk_mcp_manifests(None));
    assert!(options.captures_sdk_mcp_manifests(Some(std::ffi::OsStr::new("0"))));
    assert!(options.captures_sdk_mcp_manifests(Some(std::ffi::OsStr::new(""))));
    for value in ["1", " TRUE ", "Yes", "on"] {
        assert!(
            !options.captures_sdk_mcp_manifests(Some(std::ffi::OsStr::new(value))),
            "{value}"
        );
    }
    let forced = trogon_claude_agent_sdk::ClaudeAgentOptions {
        capture_sdk_mcp_manifests: Some(true),
        ..trogon_claude_agent_sdk::ClaudeAgentOptions::default()
    };
    assert!(forced.captures_sdk_mcp_manifests(Some(std::ffi::OsStr::new("1"))));
}
