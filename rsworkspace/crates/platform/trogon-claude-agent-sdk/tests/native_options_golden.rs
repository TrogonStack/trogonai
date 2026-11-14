#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests answer the initialize request through a scripted child process."
    )
)]

//! Proves every option of the TypeScript `Options` (0.3.323) and the Python `ClaudeAgentOptions` (0.2.181) reaches
//! the CLI in the same form from the Rust options. Each case in the fixture was recorded from the real
//! `sdk.mjs` and `claude-agent-sdk` packages driving a recording stand-in for the CLI: the full argv, the
//! `initialize` request text, the environment entries that differ from a run with no option, whether user
//! messages are marked `client_composed`, and the spawn command where the option changes it.

use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    AgentDefinition, AgentName, ArgvStyle, CallbackError, ClaudeAgentOptions, ClaudeSdkClient, ConfigValue,
    ContinueConversation, DangerousPermissionSkip, DebugOutput, DialogKind, EffortLevel, EmptyCollection, Environment,
    EnvironmentMode, ExtraArgument, FileCheckpointing, HookEvent, HookEvents, HookHandler, HookOutput,
    HookRegistration, HookRequest, InMemorySessionStore, LocalMcpServer, LocalMcpServerOptions, LocalPlugin,
    McpDiscovery, McpRemoteServerOptions, McpServerDialect, McpServerName, McpServerOptions, McpStdioServerOptions,
    ModelName, OutputSchema, PartialMessages, PermissionDecision, PermissionMode, PermissionPrompts, PluginDelivery,
    Prompt, QuestionPreviewFormat, SandboxSettings, ScriptRuntime, SdkBeta, SdkMcpServer, SdkTools, SessionFork,
    SessionPersistence, SessionStore, SessionStoreFlush, SessionStoreOptions, SettingSource, Settings, SettingsInput,
    Skills, SystemPrompt, TaskBudget, ThinkingConfig, ThinkingDisplay, Timeout, ToolName, ToolPermissionRequest,
    ToolRule, ToolSet, UsdBudget, UserDialogRequest, UserDialogResponse, VerbatimPrompts,
    adapters::{NativeFileSystem, RuntimeAdapters, SystemClock},
    test_support::FixedEnvironment,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;

use scripted_process::{ScriptedProcessHarness, ScriptedProcessSpawner};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Build = Result<(), Box<dyn std::error::Error>>;

const GOLDENS: &str = include_str!("fixtures/native_options/upstream.json");

const SESSION: &str = "11111111-1111-4111-8111-111111111111";
const OTHER_SESSION: &str = "22222222-2222-4222-8222-222222222222";
const SETTINGS_OBJECT: &str = r#"{"model":"opus","permissions":{"allow":["Bash(ls)"]}}"#;
const SANDBOX: &str =
    r#"{"enabled":true,"network":{"allowedDomains":["x.com"],"httpProxyPort":1},"filesystem":{"denyRead":["/a"]}}"#;
const SCHEMA: &str = r#"{"type":"object","properties":{"a":{"type":"string"}}}"#;

/// Test files that hold the cases a `covered_elsewhere` entry points at.
const SOURCES: [(&str, &str); 8] = [
    ("tests/shutdown_errors.rs", include_str!("shutdown_errors.rs")),
    ("tests/session_store.rs", include_str!("session_store.rs")),
    ("tests/runtime_features.rs", include_str!("runtime_features.rs")),
    ("tests/runtime_adapters.rs", include_str!("runtime_adapters.rs")),
    ("tests/core_options_golden.rs", include_str!("core_options_golden.rs")),
    ("tests/subprocess.rs", include_str!("subprocess.rs")),
    (
        "src/process_transport/run_as_tests.rs",
        include_str!("../src/process_transport/run_as_tests.rs"),
    ),
    (
        "tests/native_options_golden.rs",
        include_str!("native_options_golden.rs"),
    ),
];

/// What one run of the options left on the child process and on the first `initialize` request.
#[derive(Debug, PartialEq)]
struct Observation {
    executable: String,
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
    directory: Option<PathBuf>,
    request: String,
    composed: bool,
}

async fn serve(process: ScriptedProcessHarness) -> Result<(String, bool), std::io::Error> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let stderr = process.stderr;
    let mut request = String::new();
    let mut composed = false;
    while let Some(line) = lines.next_line().await? {
        let frame: Value = serde_json::from_str(&line)?;
        if frame["request"]["subtype"] == "initialize" {
            let mut response = serde_json::to_vec(&json!({"type":"control_response","response":{
                "subtype":"success","request_id":frame["request_id"],
                "response":{"hooks_applied":true,"hook_on_failure_supported":true,"claude_code_version":"2.1.319"}}}))?;
            response.push(b'\n');
            stdout.write_all(&response).await?;
            request = serde_json::to_string(&frame["request"])?;
        }
        if frame["type"] == "user" {
            composed = frame["client_composed"] == true;
        }
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok((request, composed))
}

async fn observe(options: ClaudeAgentOptions) -> Result<Observation, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let server = tokio::spawn(serve(harness));
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
    client.send_prompt(Prompt::from("hi")).await?;
    client.disconnect().await?;
    let (request, composed) = server.await??;
    let command = observer.commands()?.remove(0);
    Ok(Observation {
        executable: command.executable.to_string_lossy().into_owned(),
        arguments: command
            .arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect(),
        environment: command
            .environment
            .iter()
            .filter_map(|(key, value)| {
                value
                    .as_ref()
                    .map(|value| (key.to_string_lossy().into_owned(), value.to_string_lossy().into_owned()))
            })
            .collect(),
        directory: command.directory,
        request,
        composed,
    })
}

fn base(style: ArgvStyle) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        argv_style: style,
        environment_mode: match style {
            ArgvStyle::TypeScript => EnvironmentMode::default(),
            ArgvStyle::Python => EnvironmentMode::Overlay,
        },
        ..ClaudeAgentOptions::default()
    }
}

/// The entries of `case` that differ from `baseline`, with `null` for an entry the case removed.
fn environment_delta(baseline: &BTreeMap<String, String>, case: &BTreeMap<String, String>) -> Value {
    let mut delta = Map::new();
    for (key, value) in case {
        if baseline.get(key) != Some(value) {
            delta.insert(key.clone(), Value::String(value.clone()));
        }
    }
    for key in baseline.keys() {
        if !case.contains_key(key) {
            delta.insert(key.clone(), Value::Null);
        }
    }
    Value::Object(delta)
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn directories(items: &[&str]) -> Vec<PathBuf> {
    items.iter().map(PathBuf::from).collect()
}

fn rules(items: &[&str]) -> Vec<ToolRule> {
    items.iter().map(|item| (*item).into()).collect()
}

fn tools(items: &[&str]) -> ToolSet {
    ToolSet::Only(items.iter().map(|item| (*item).into()).collect())
}

fn extras(items: &[(&str, Option<&str>)]) -> Vec<ExtraArgument> {
    items
        .iter()
        .map(|(flag, value)| ExtraArgument {
            flag: (*flag).into(),
            value: value.map(str::to_owned),
        })
        .collect()
}

fn noop_hook() -> Arc<dyn HookHandler> {
    Arc::new(|_: HookRequest| async { Ok::<HookOutput, CallbackError>(HookOutput::default()) })
}

fn hook_registrations() -> Result<Vec<HookRegistration>, Box<dyn std::error::Error>> {
    Ok(vec![
        HookRegistration {
            event: HookEvent::PreToolUse,
            matcher: Some("Bash".into()),
            callbacks: vec![noop_hook(), noop_hook()],
            timeout: Some(Timeout::checked(Duration::from_secs(30))?),
            on_failure: None,
        },
        HookRegistration {
            event: HookEvent::PostToolUse,
            matcher: None,
            callbacks: vec![noop_hook()],
            timeout: None,
            on_failure: None,
        },
    ])
}

fn allow_everything(options: &mut ClaudeAgentOptions) {
    options.permission_handler = Some(Arc::new(|_: ToolPermissionRequest| async {
        PermissionDecision::allow()
    }));
}

fn cancel_dialogs(options: &mut ClaudeAgentOptions) {
    options.user_dialog_handler = Some(Arc::new(|_: UserDialogRequest| async {
        Ok::<UserDialogResponse, CallbackError>(UserDialogResponse::Cancelled)
    }));
}

fn external_servers(options: &mut ClaudeAgentOptions) {
    options.mcp_servers.insert(
        McpServerName::from("local"),
        McpServerOptions::Stdio(McpStdioServerOptions {
            command: PathBuf::from("server"),
            args: Some(vec!["--flag".into()]),
            env: Some(Environment::from([("K".to_owned(), ConfigValue::from("v"))])),
            ..McpStdioServerOptions::default()
        }),
    );
    let mut remote = McpRemoteServerOptions::new("https://example.com/mcp");
    remote.headers = Some(BTreeMap::from([("A".to_owned(), ConfigValue::from("b"))]));
    options
        .mcp_servers
        .insert(McpServerName::from("remote"), McpServerOptions::Http(remote));
    options.mcp_servers.insert(
        McpServerName::from("events"),
        McpServerOptions::Sse(McpRemoteServerOptions::new("https://example.com/sse")),
    );
}

fn in_process_server(options: &mut ClaudeAgentOptions, dialect: McpServerDialect) {
    let server: Arc<dyn SdkMcpServer> = Arc::new(LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from("inproc"),
        version: Some("1.0.0".into()),
        tools: Some(SdkTools::default()),
        instructions: None,
        always_load: false,
        dialect,
    }));
    options.sdk_mcp_servers.insert(McpServerName::from("inproc"), server);
}

fn agents(options: &mut ClaudeAgentOptions, every_field: bool) {
    let mut reviewer = AgentDefinition::new("d", Prompt::from("p"));
    if every_field {
        reviewer.tools = Some(vec![ToolName::from("Read"), ToolName::from("Edit")]);
        reviewer.disallowed_tools = Some(vec![ToolName::from("Bash")]);
        reviewer.model = Some(ModelName::from("opus"));
        reviewer.skills = Some(vec!["s1".to_owned()]);
        reviewer.max_turns = NonZeroUsize::new(3);
        reviewer.effort = Some(EffortLevel::High);
    }
    options.agents.insert(AgentName::from("rev"), reviewer);
    if every_field {
        options
            .agents
            .insert(AgentName::from("second"), AgentDefinition::new("o", Prompt::from("q")));
    }
}

fn store_options(flush: SessionStoreFlush) -> SessionStoreOptions {
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::<SystemClock>::default());
    let mut options = SessionStoreOptions::from(store);
    options.flush = flush;
    options
}

fn output_schema() -> Result<OutputSchema, Box<dyn std::error::Error>> {
    Ok(OutputSchema::new(serde_json::from_str(SCHEMA)?))
}

fn permission_mode(options: &mut ClaudeAgentOptions, mode: PermissionMode) {
    options.permission_mode = Some(mode);
}

fn effort(options: &mut ClaudeAgentOptions, level: EffortLevel) {
    options.effort = Some(level);
}

fn unknown(name: &str) -> Build {
    Err(format!("no Rust options are defined for the case {name}").into())
}

/// The Rust options equivalent to each TypeScript `Options` case of the fixture.
fn typescript_options(name: &str, o: &mut ClaudeAgentOptions) -> Build {
    match name {
        "baseline"
        | "additionalDirectories_empty"
        | "allowDangerouslySkipPermissions_false"
        | "allowedTools_empty"
        | "betas_empty"
        | "continue_false"
        | "debug_false"
        | "enableFileCheckpointing_false"
        | "forkSession_false"
        | "includeHookEvents_false"
        | "includePartialMessages_false"
        | "mcpServers_empty"
        | "persistSession_true"
        | "sessionStoreFlush_alone"
        | "strictMcpConfig_false"
        | "toolConfig_empty"
        | "verbatimPrompts_false" => {}
        "additionalDirectories" => o.additional_directories = directories(&["/a", "/b c"]),
        "agent" => o.agent = Some("reviewer".into()),
        "agentProgressSummaries_true" => o.agent_progress_summaries = Some(true),
        "agentProgressSummaries_false" => o.agent_progress_summaries = Some(false),
        "agents_basic" => agents(o, false),
        "agents_fields" => agents(o, true),
        "allowDangerouslySkipPermissions_true" => {
            o.allow_dangerously_skip_permissions = DangerousPermissionSkip::Allow;
        }
        "allowedTools" => o.allowed_tools = rules(&["Read", "Bash(ls:*)", "mcp__s__t"]),
        "betas" => o.betas = vec![SdkBeta::Context1m],
        "canUseTool" => allow_everything(o),
        "continue_true" => o.continue_conversation = ContinueConversation::Yes,
        "cwd" => o.cwd = Some("work".into()),
        "debug_true" => o.debug = DebugOutput::On,
        "debugFile" => o.debug_file = Some("debug.log".into()),
        "debugFile_with_debug" => {
            o.debug = DebugOutput::On;
            o.debug_file = Some("debug.log".into());
        }
        "disallowedTools" => o.disallowed_tools = rules(&["Write", "Bash(rm:*)"]),
        "effort_low" => effort(o, EffortLevel::Low),
        "effort_medium" => effort(o, EffortLevel::Medium),
        "effort_high" => effort(o, EffortLevel::High),
        "effort_xhigh" => effort(o, EffortLevel::XHigh),
        "effort_max" => effort(o, EffortLevel::Max),
        "enableFileCheckpointing_true" => o.file_checkpointing = FileCheckpointing::Enabled,
        "env" => {
            o.env.insert("FOO".into(), "bar".into());
            o.env.insert("EMPTY".into(), "".into());
        }
        "executable" => {
            o.cli_path = Some("cli.mjs".into());
            o.script_runtime = ScriptRuntime {
                executable: Some("node-runtime".into()),
                arguments: Vec::new(),
            };
        }
        "executableArgs" => {
            o.cli_path = Some("cli.mjs".into());
            o.script_runtime = ScriptRuntime {
                executable: Some("node-runtime".into()),
                arguments: vec!["--no-warnings".into(), "--stack-size=2000".into()],
            };
        }
        "pathToClaudeCodeExecutable" => o.cli_path = Some("claude-bin".into()),
        "extraArgs" => {
            o.extra_args = extras(&[
                ("bare-flag", None),
                ("spaced", Some("value")),
                ("dashed", Some("-1")),
                ("empty", Some("")),
            ]);
        }
        "fallbackModel" => o.fallback_model = Some(ModelName::from("sonnet")),
        "forkSession_true" => o.fork_session = SessionFork::Fork,
        "forwardSubagentText_true" => o.forward_subagent_text = Some(true),
        "forwardSubagentText_false" => o.forward_subagent_text = Some(false),
        "hooks" => o.hooks = hook_registrations()?,
        "hooks_empty_list" => o.hooks_when_empty = EmptyCollection::Sent,
        "includeHookEvents_true" => o.include_hook_events = HookEvents::Include,
        "includePartialMessages_true" => o.partial_messages = PartialMessages::Include,
        "managedSettings" => o.managed_settings = Some(Settings::from_json(SETTINGS_OBJECT)?),
        "maxBudgetUsd" => o.max_budget_usd = Some(UsdBudget::new(1.5)),
        "maxBudgetUsd_zero" => o.max_budget_usd = Some(UsdBudget::new(0.0)),
        "maxThinkingTokens" => o.max_thinking_tokens = Some(1000),
        "maxThinkingTokens_zero" => o.max_thinking_tokens = Some(0),
        "maxTurns" => o.max_turns = NonZeroUsize::new(7),
        "mcpServers_external" => external_servers(o),
        "mcpServers_sdk" => in_process_server(o, McpServerDialect::TypeScript),
        "model" => o.model = Some(ModelName::from("opus")),
        "outputFormat" => o.output_schema = Some(output_schema()?),
        "perTaskStopAffordance_true" => o.per_task_stop_affordance = Some(true),
        "perTaskStopAffordance_false" => o.per_task_stop_affordance = Some(false),
        "permissionMode_default" => permission_mode(o, PermissionMode::Default),
        "permissionMode_acceptEdits" => permission_mode(o, PermissionMode::AcceptEdits),
        "permissionMode_plan" => permission_mode(o, PermissionMode::Plan),
        "permissionMode_bypassPermissions" => permission_mode(o, PermissionMode::BypassPermissions),
        "permissionMode_dontAsk" => permission_mode(o, PermissionMode::DontAsk),
        "permissionMode_auto" => permission_mode(o, PermissionMode::Auto),
        "permissionPromptToolName" => o.permission_prompt_tool_name = Some("mcp__x__approve".into()),
        "permissionPrompts_host" => o.permission_prompts = Some(PermissionPrompts::Host),
        "permissionPrompts_none" => o.permission_prompts = Some(PermissionPrompts::None),
        "persistSession_false" => o.session_persistence = SessionPersistence::Disabled,
        "planModeInstructions" => o.plan_mode_instructions = Some("plan it".into()),
        "plugins" | "pluginDelivery_argv" => {
            let mut second = LocalPlugin::new("plug2");
            second.skip_mcp_discovery = Some(true);
            o.plugins = vec![LocalPlugin::new("plug1"), second];
            o.plugin_delivery = PluginDelivery::Argv;
        }
        "pluginDelivery_initialize" => {
            let mut second = LocalPlugin::new("plug2");
            second.skip_mcp_discovery = Some(true);
            o.plugins = vec![LocalPlugin::new("plug1"), second];
            o.plugin_delivery = PluginDelivery::Initialize;
        }
        "pluginDelivery_initialize_no_plugins" => o.plugin_delivery = PluginDelivery::Initialize,
        "projectConfigRoot" => o.project_config_root = Some("cfgroot".into()),
        "promptSuggestions_true" => o.prompt_suggestions = Some(true),
        "promptSuggestions_false" => o.prompt_suggestions = Some(false),
        "resume" => o.resume = Some(SESSION.into()),
        "resumeDropsTurn" => {
            o.resume = Some(SESSION.into());
            o.resume_drops_turn = Some(SESSION.to_owned());
        }
        "resumeSessionAt" => {
            o.resume = Some(SESSION.into());
            o.resume_session_at = Some(OTHER_SESSION.to_owned());
        }
        "sandbox" => o.sandbox = Some(serde_json::from_str::<SandboxSettings>(SANDBOX)?),
        "sandbox_disabled" => o.sandbox = Some(serde_json::from_str::<SandboxSettings>(r#"{"enabled":false}"#)?),
        "sessionId" => o.session_id = Some(SESSION.into()),
        "sessionStore" => o.session_store = Some(store_options(SessionStoreFlush::default())),
        "sessionStoreFlush_eager" => o.session_store = Some(store_options(SessionStoreFlush::Eager)),
        "sessionStoreFlush_batched" => o.session_store = Some(store_options(SessionStoreFlush::Batched)),
        "settingSources" => {
            o.setting_sources = Some(vec![SettingSource::User, SettingSource::Project, SettingSource::Local]);
        }
        "settingSources_empty" => o.setting_sources = Some(Vec::new()),
        "settings_path" => o.settings = Some(SettingsInput::path("settings_file.json")),
        "settings_json" => o.settings = Some(SettingsInput::json(r#"{"a": 1, "b": [1, 2]}"#)),
        "settings_object" => o.settings = Some(SettingsInput::inline(Settings::from_json(SETTINGS_OBJECT)?)),
        "skills_all" => o.skills = Some(Skills::All),
        "skills_named" => o.skills = Some(Skills::Named(vec!["a".to_owned(), "b".to_owned()])),
        "skills_empty" => o.skills = Some(Skills::Named(Vec::new())),
        "strictMcpConfig_true" => o.mcp_discovery = McpDiscovery::Disabled,
        "supportedDialogKinds" => {
            cancel_dialogs(o);
            o.supported_dialog_kinds = vec![DialogKind::from("refusal_fallback_prompt"), DialogKind::from("k2")];
        }
        "supportedDialogKinds_empty" => {
            cancel_dialogs(o);
            o.supported_dialog_kinds_when_empty = EmptyCollection::Sent;
        }
        "systemPrompt_string" => o.system_prompt = SystemPrompt::Custom("be brief".into()),
        "systemPrompt_preset" => o.system_prompt = SystemPrompt::ClaudeCode,
        "systemPrompt_preset_append" => o.system_prompt = SystemPrompt::ClaudeCodeWithAppend("more".into()),
        "systemPrompt_array" => o.system_prompt = SystemPrompt::Parts(vec!["a".to_owned(), "b".to_owned()]),
        "taskBudget" => o.task_budget = Some(TaskBudget { total: 5000 }),
        "thinking_adaptive" => o.thinking = Some(ThinkingConfig::Adaptive { display: None }),
        "thinking_adaptive_display" => {
            o.thinking = Some(ThinkingConfig::Adaptive {
                display: Some(ThinkingDisplay::Summarized),
            });
        }
        "thinking_enabled" => {
            o.thinking = Some(ThinkingConfig::Enabled {
                budget_tokens: Some(2000),
                display: None,
            });
        }
        "thinking_enabled_display" => {
            o.thinking = Some(ThinkingConfig::Enabled {
                budget_tokens: Some(2000),
                display: Some(ThinkingDisplay::Omitted),
            });
        }
        "thinking_enabled_no_budget" => {
            o.thinking = Some(ThinkingConfig::Enabled {
                budget_tokens: None,
                display: None,
            });
        }
        "thinking_disabled" => o.thinking = Some(ThinkingConfig::Disabled),
        "title" => o.title = Some("My title".into()),
        "toolAliases" => {
            o.tool_aliases = Some(vec![
                ("Bash".to_owned(), "mcp__w__bash".to_owned()),
                ("Read".to_owned(), "mcp__w__read".to_owned()),
            ]);
        }
        "toolConfig_html" => o.tool_config.ask_user_question.preview_format = Some(QuestionPreviewFormat::Html),
        "toolConfig_markdown" => {
            o.tool_config.ask_user_question.preview_format = Some(QuestionPreviewFormat::Markdown);
        }
        "tools_list" => o.tools = tools(&["Read", "Edit"]),
        "tools_empty" => o.tools = tools(&[]),
        "tools_preset" => o.tools = ToolSet::ClaudeCodePreset,
        "verbatimPrompts_true" => o.verbatim_prompts = VerbatimPrompts::Enabled,
        other => return unknown(other),
    }
    Ok(())
}

/// The Rust options equivalent to each Python `ClaudeAgentOptions` case of the fixture.
fn python_options(name: &str, o: &mut ClaudeAgentOptions) -> Build {
    match name {
        "baseline"
        | "add_dirs_empty"
        | "allowed_tools_empty"
        | "betas_empty"
        | "continue_conversation_false"
        | "enable_file_checkpointing_false"
        | "fork_session_false"
        | "hooks_empty_list"
        | "hooks_empty_dict"
        | "include_hook_events_false"
        | "include_partial_messages_false"
        | "mcp_servers_empty"
        | "session_store_flush_alone"
        | "strict_mcp_config_false"
        | "verbatim_prompts_false" => {}
        "add_dirs" => o.additional_directories = directories(&["/a", "/b c"]),
        "agents_basic" => agents(o, false),
        "agents_fields" => agents(o, true),
        "allowed_tools" => o.allowed_tools = rules(&["Read", "Bash(ls:*)", "mcp__s__t"]),
        "betas" => o.betas = vec![SdkBeta::Context1m],
        "can_use_tool" => allow_everything(o),
        "cli_path" => o.cli_path = Some("claude-alt".into()),
        "client_app" => o.client_app = Some("my-app".into()),
        "continue_conversation_true" => o.continue_conversation = ContinueConversation::Yes,
        "cwd" => o.cwd = Some("work".into()),
        "disallowed_tools" => o.disallowed_tools = rules(&["Write", "Bash(rm:*)"]),
        "effort_low" => effort(o, EffortLevel::Low),
        "effort_medium" => effort(o, EffortLevel::Medium),
        "effort_high" => effort(o, EffortLevel::High),
        "effort_xhigh" => effort(o, EffortLevel::XHigh),
        "effort_max" => effort(o, EffortLevel::Max),
        "enable_file_checkpointing_true" => o.file_checkpointing = FileCheckpointing::Enabled,
        "env" => {
            o.env.insert("FOO".into(), "bar".into());
            o.env.insert("EMPTY".into(), "".into());
        }
        "extra_args" => {
            o.extra_args = extras(&[
                ("bare-flag", None),
                ("spaced", Some("value")),
                ("dashed", Some("-1")),
                ("empty", Some("")),
            ]);
        }
        "fallback_model" => o.fallback_model = Some(ModelName::from("sonnet")),
        "fork_session_true" => o.fork_session = SessionFork::Fork,
        "forward_subagent_text_true" => o.forward_subagent_text = Some(true),
        "forward_subagent_text_false" => o.forward_subagent_text = Some(false),
        "hooks" => o.hooks = hook_registrations()?,
        "include_hook_events_true" => o.include_hook_events = HookEvents::Include,
        "include_partial_messages_true" => o.partial_messages = PartialMessages::Include,
        "max_budget_usd" => o.max_budget_usd = Some(UsdBudget::new(1.5)),
        "max_budget_usd_zero" => o.max_budget_usd = Some(UsdBudget::new(0.0)),
        "max_thinking_tokens" => o.max_thinking_tokens = Some(1000),
        "max_thinking_tokens_zero" => o.max_thinking_tokens = Some(0),
        "max_turns" => o.max_turns = NonZeroUsize::new(7),
        "mcp_servers_external" => external_servers(o),
        "mcp_servers_sdk" => in_process_server(o, McpServerDialect::Python),
        "model" => o.model = Some(ModelName::from("opus")),
        "output_format" => o.output_schema = Some(output_schema()?),
        "permission_mode_default" => permission_mode(o, PermissionMode::Default),
        "permission_mode_acceptEdits" => permission_mode(o, PermissionMode::AcceptEdits),
        "permission_mode_plan" => permission_mode(o, PermissionMode::Plan),
        "permission_mode_bypassPermissions" => permission_mode(o, PermissionMode::BypassPermissions),
        "permission_mode_dontAsk" => permission_mode(o, PermissionMode::DontAsk),
        "permission_mode_auto" => permission_mode(o, PermissionMode::Auto),
        "permission_prompt_tool_name" => o.permission_prompt_tool_name = Some("mcp__x__approve".into()),
        "plugins" => o.plugins = vec![LocalPlugin::new("plug1"), LocalPlugin::new("plug2")],
        "resume" => o.resume = Some(SESSION.into()),
        "resume_drops_turn" => {
            o.resume = Some(SESSION.into());
            o.resume_drops_turn = Some(SESSION.to_owned());
        }
        "resume_session_at" => {
            o.resume = Some(SESSION.into());
            o.resume_session_at = Some(OTHER_SESSION.to_owned());
        }
        "sandbox" => o.sandbox = Some(serde_json::from_str::<SandboxSettings>(SANDBOX)?),
        "sandbox_disabled" => o.sandbox = Some(serde_json::from_str::<SandboxSettings>(r#"{"enabled":false}"#)?),
        "session_id" => o.session_id = Some(SESSION.into()),
        "session_store" => o.session_store = Some(store_options(SessionStoreFlush::default())),
        "session_store_flush_eager" => o.session_store = Some(store_options(SessionStoreFlush::Eager)),
        "session_store_flush_batched" => o.session_store = Some(store_options(SessionStoreFlush::Batched)),
        "setting_sources" => {
            o.setting_sources = Some(vec![SettingSource::User, SettingSource::Project, SettingSource::Local]);
        }
        "setting_sources_empty" => o.setting_sources = Some(Vec::new()),
        "settings_path" => o.settings = Some(SettingsInput::path("settings_file.json")),
        "settings_json" => o.settings = Some(SettingsInput::json(r#"{"a": 1, "b": [1, 2]}"#)),
        "skills_all" => o.skills = Some(Skills::All),
        "skills_named" => o.skills = Some(Skills::Named(vec!["a".to_owned(), "b".to_owned()])),
        "skills_empty" => o.skills = Some(Skills::Named(Vec::new())),
        "strict_mcp_config_true" => o.mcp_discovery = McpDiscovery::Disabled,
        "system_prompt_string" => o.system_prompt = SystemPrompt::Custom("be brief".into()),
        "system_prompt_empty" => o.system_prompt = SystemPrompt::Custom(String::new()),
        "system_prompt_preset" => o.system_prompt = SystemPrompt::ClaudeCode,
        "system_prompt_preset_append" => o.system_prompt = SystemPrompt::ClaudeCodeWithAppend("more".into()),
        "system_prompt_file" => o.system_prompt = SystemPrompt::File("prompt.txt".into()),
        "task_budget" => o.task_budget = Some(TaskBudget { total: 5000 }),
        "thinking_adaptive" => o.thinking = Some(ThinkingConfig::Adaptive { display: None }),
        "thinking_adaptive_display" => {
            o.thinking = Some(ThinkingConfig::Adaptive {
                display: Some(ThinkingDisplay::Summarized),
            });
        }
        "thinking_enabled" => {
            o.thinking = Some(ThinkingConfig::Enabled {
                budget_tokens: Some(2000),
                display: None,
            });
        }
        "thinking_enabled_display" => {
            o.thinking = Some(ThinkingConfig::Enabled {
                budget_tokens: Some(2000),
                display: Some(ThinkingDisplay::Omitted),
            });
        }
        "thinking_disabled" => o.thinking = Some(ThinkingConfig::Disabled),
        "tools_list" => o.tools = tools(&["Read", "Edit"]),
        "tools_empty" => o.tools = tools(&[]),
        "tools_preset" => o.tools = ToolSet::ClaudeCodePreset,
        "verbatim_prompts_true" => o.verbatim_prompts = VerbatimPrompts::Enabled,
        other => return unknown(other),
    }
    Ok(())
}

/// Runs every fixture case of one dialect and reports all the cases that differ, not only the first.
async fn check_dialect(
    dialect: &str,
    style: ArgvStyle,
    build: fn(&str, &mut ClaudeAgentOptions) -> Build,
) -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let cases = goldens[dialect]
        .as_object()
        .ok_or("the fixture has no cases for the dialect")?;
    let baseline = observe(base(style)).await?;
    let mut failures: Vec<String> = Vec::new();
    for (name, golden) in cases {
        let mut options = base(style);
        if let Err(error) = build(name, &mut options) {
            failures.push(format!("{dialect} {name}: {error}"));
            continue;
        }
        let cli = options
            .cli_path
            .as_deref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let node = options
            .script_runtime
            .executable
            .as_deref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let seen = match observe(options).await {
            Ok(seen) => seen,
            Err(error) => {
                failures.push(format!("{dialect} {name}: run failed: {error}"));
                continue;
            }
        };
        let want_argv = strings(&golden["argv"]);
        match golden.get("spawn") {
            Some(spawn) => {
                let token = |text: &str| match text {
                    "<node>" => node.clone(),
                    "<cli>" => cli.clone(),
                    other => other.to_owned(),
                };
                let want_executable = token(spawn["command"].as_str().unwrap_or_default());
                let want_arguments: Vec<String> = strings(&spawn["args"]).iter().map(|text| token(text)).collect();
                if seen.executable != want_executable {
                    failures.push(format!(
                        "{dialect} {name}: executable {:?}, upstream {want_executable:?}",
                        seen.executable
                    ));
                }
                if seen.arguments != want_arguments {
                    failures.push(format!(
                        "{dialect} {name}: spawn arguments {:?}, upstream {want_arguments:?}",
                        seen.arguments
                    ));
                }
                if !seen.arguments.ends_with(&want_argv) {
                    failures.push(format!(
                        "{dialect} {name}: argv {:?} does not end with {want_argv:?}",
                        seen.arguments
                    ));
                }
            }
            None => {
                if seen.arguments != want_argv {
                    failures.push(format!(
                        "{dialect} {name}: argv {:?}, upstream {want_argv:?}",
                        seen.arguments
                    ));
                }
            }
        }
        let want_request = golden["request"].as_str().unwrap_or_default();
        if seen.request != want_request {
            failures.push(format!(
                "{dialect} {name}: initialize {:?}, upstream {want_request:?}",
                seen.request
            ));
        }
        let delta = environment_delta(&baseline.environment, &seen.environment);
        if delta != golden["env"] {
            failures.push(format!(
                "{dialect} {name}: environment {delta}, upstream {}",
                golden["env"]
            ));
        }
        let want_composed = golden["user_client_composed"].as_bool().unwrap_or(false);
        if seen.composed != want_composed {
            failures.push(format!(
                "{dialect} {name}: client_composed {}, upstream {want_composed}",
                seen.composed
            ));
        }
        let want_directory = golden.get("directory").and_then(Value::as_str);
        if seen.directory.as_deref().and_then(Path::to_str) != want_directory {
            failures.push(format!(
                "{dialect} {name}: directory {:?}, upstream {want_directory:?}",
                seen.directory
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} case(s) differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn every_typescript_option_matches_the_upstream_wire_form() -> TestResult {
    check_dialect("typescript", ArgvStyle::TypeScript, typescript_options).await
}

#[tokio::test]
async fn every_python_option_matches_the_upstream_wire_form() -> TestResult {
    check_dialect("python", ArgvStyle::Python, python_options).await
}

/// Python's `debug_stderr` only redirects the CLI's debug output to a file object, so nothing reaches the CLI.
#[tokio::test]
async fn python_debug_stderr_is_accepted_without_changing_the_wire() -> TestResult {
    let plain = observe(base(ArgvStyle::Python)).await?;
    let mut options = base(ArgvStyle::Python);
    options.debug_stderr = true;
    let debug = observe(options).await?;
    assert_eq!(plain, debug);
    Ok(())
}

fn case_options(goldens: &Value, dialect: &str) -> Vec<String> {
    goldens[dialect]
        .as_object()
        .map(|cases| {
            cases
                .values()
                .filter_map(|case| case["option"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn covered_by_an_existing_test(location: &str) -> bool {
    let Some((file, name)) = location.split_once("::") else {
        return false;
    };
    SOURCES
        .iter()
        .any(|(path, source)| *path == file && source.contains(&format!("fn {name}")))
}

#[test]
fn every_upstream_option_has_a_golden_case() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let mut problems: Vec<String> = Vec::new();
    for dialect in ["typescript", "python"] {
        let keys = strings(&goldens["upstream_keys"][dialect]);
        assert!(!keys.is_empty(), "the fixture lists no upstream keys for {dialect}");
        let with_case = case_options(&goldens, dialect);
        for key in &keys {
            let elsewhere = goldens["covered_elsewhere"][dialect][key].as_str();
            match (with_case.contains(key), elsewhere) {
                (true, None) => {}
                (false, Some(location)) if covered_by_an_existing_test(location) => {}
                (false, Some(location)) => problems.push(format!("{dialect} {key}: no test at {location}")),
                (true, Some(_)) => problems.push(format!("{dialect} {key}: both a case and covered_elsewhere")),
                (false, None) => problems.push(format!("{dialect} {key}: no case and no covering test")),
            }
        }
        for option in &with_case {
            if !keys.contains(option) {
                problems.push(format!(
                    "{dialect} {option}: a case names an option upstream does not have"
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    Ok(())
}
