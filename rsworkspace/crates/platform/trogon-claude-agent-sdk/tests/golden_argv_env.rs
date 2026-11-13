#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Golden tests that pin the argv, environment and initialize bytes the SDK produces.
//! The expected tokens come from `sdk.mjs` 0.3.323 and `subprocess_cli.py` 0.2.181.

use serde_json::{Value, json};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    ArgvStyle, ClaudeAgentOptions, ClaudeCodeMarker, ClaudeSdkClient, Environment, EnvironmentMode, ExtraArgument,
    HookEvent, HookOutput, HookRegistration, LocalMcpServer, LocalMcpServerOptions, McpDiscovery,
    McpRemoteServerOptions, McpSdkServerOptions, McpServerDialect, McpServerName, McpServerOptions,
    McpServerToolPolicy, McpStdioServerOptions, McpToolPermissionPolicy, ScriptRuntime, SdkMcpServer, SdkTools,
    SystemPrompt, TraceContext, UsdBudget,
    adapters::{NativeFileSystem, RuntimeAdapters},
    test_support::FixedEnvironment,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;

use scripted_process::{CapturedProcessCommand, ScriptedProcessHarness, ScriptedProcessSpawner};

struct Capture {
    command: CapturedProcessCommand,
    initialize: Value,
}

async fn serve_initialize(process: ScriptedProcessHarness) -> Result<Value, std::io::Error> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let stderr = process.stderr;
    let mut first = None;
    while let Some(line) = lines.next_line().await? {
        let request: Value = serde_json::from_str(&line)?;
        if request["request"]["subtype"] == "initialize" {
            let mut frame = serde_json::to_vec(&json!({"type":"control_response","response":{
                "subtype":"success","request_id":request["request_id"],
                "response":{"hooks_applied":true,"hook_on_failure_supported":true,"claude_code_version":"2.1.319"}}}))?;
            frame.push(b'\n');
            stdout.write_all(&frame).await?;
            first = Some(request);
        }
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    first.ok_or_else(|| std::io::Error::other("no initialize request"))
}

async fn run(options: ClaudeAgentOptions, variables: &[(&str, &str)]) -> Result<Capture, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let server = tokio::spawn(serve_initialize(harness));
    let variables: BTreeMap<OsString, OsString> = variables.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
    let client = ClaudeSdkClient::connect_with_adapters(
        options,
        RuntimeAdapters {
            environment: FixedEnvironment {
                directory: PathBuf::from("unused-directory"),
                variables,
            },
            process,
            filesystem: NativeFileSystem,
        },
    )
    .await?;
    client.disconnect().await?;
    let initialize = server.await??;
    let command = observer.commands()?.remove(0);
    Ok(Capture { command, initialize })
}

fn base() -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        ..ClaudeAgentOptions::default()
    }
}

fn arguments(capture: &Capture) -> Vec<String> {
    capture
        .command
        .arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

fn environment(capture: &Capture, key: &str) -> Option<Option<String>> {
    capture
        .command
        .environment
        .get(&OsString::from(key))
        .map(|value| value.as_ref().map(|value| value.to_string_lossy().into_owned()))
}

#[tokio::test]
async fn typescript_default_argv_has_no_strict_mcp_or_permission_tool_and_sends_prompt_in_initialize()
-> Result<(), Box<dyn std::error::Error>> {
    let capture = run(base(), &[]).await?;
    assert_eq!(
        arguments(&capture),
        [
            "--output-format",
            "stream-json",
            "--verbose",
            "--input-format",
            "stream-json"
        ]
    );
    let request = &capture.initialize["request"];
    assert_eq!(request["systemPrompt"], json!([""]));
    assert!(request.get("appendSystemPrompt").is_none());
    Ok(())
}

#[tokio::test]
async fn typescript_flags_use_equals_and_follow_upstream_order() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.model = Some("opus".into());
    options.max_turns = std::num::NonZeroUsize::new(3);
    options.allowed_tools = vec!["Read".into(), "Bash".into()];
    options.continue_conversation = trogon_claude_agent_sdk::ContinueConversation::Yes;
    options.mcp_discovery = McpDiscovery::Disabled;
    options.permission_handler = Some(Arc::new(|_: trogon_claude_agent_sdk::ToolPermissionRequest| {
        Box::pin(async { trogon_claude_agent_sdk::PermissionDecision::allow() })
            as trogon_claude_agent_sdk::PermissionFuture
    }));
    let capture = run(options, &[]).await?;
    assert_eq!(
        arguments(&capture)[5..],
        [
            "--max-turns=3",
            "--model=opus",
            "--permission-prompt-tool=stdio",
            "--continue",
            "--allowedTools=Read,Bash",
            "--strict-mcp-config",
        ]
    );
    Ok(())
}

#[tokio::test]
async fn python_style_uses_spaced_flags_and_puts_the_prompt_on_argv() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.argv_style = ArgvStyle::Python;
    options.model = Some("opus".into());
    options.system_prompt = SystemPrompt::Custom("be brief".into());
    let capture = run(options, &[]).await?;
    assert_eq!(
        arguments(&capture),
        [
            "--output-format",
            "stream-json",
            "--verbose",
            "--system-prompt",
            "be brief",
            "--model",
            "opus",
            "--input-format",
            "stream-json",
        ]
    );
    assert!(capture.initialize["request"].get("systemPrompt").is_none());
    Ok(())
}

#[tokio::test]
async fn preset_prompt_with_append_travels_as_append_system_prompt() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.system_prompt = SystemPrompt::ClaudeCodeWithAppend("more".into());
    let capture = run(options, &[]).await?;
    let request = &capture.initialize["request"];
    assert!(request.get("systemPrompt").is_none());
    assert_eq!(request["appendSystemPrompt"], "more");
    Ok(())
}

#[tokio::test]
async fn extra_args_follow_upstream_dash_rule() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.extra_args = vec![
        ExtraArgument {
            flag: "bare-flag".into(),
            value: None,
        },
        ExtraArgument {
            flag: "spaced".into(),
            value: Some("value".into()),
        },
        ExtraArgument {
            flag: "dashed".into(),
            value: Some("-1".into()),
        },
    ];
    let capture = run(options, &[]).await?;
    assert_eq!(
        arguments(&capture)[5..],
        ["--bare-flag", "--spaced", "value", "--dashed=-1"]
    );
    Ok(())
}

#[tokio::test]
async fn hooks_omit_on_failure_when_unset() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.hooks = vec![HookRegistration {
        event: HookEvent::PreToolUse,
        matcher: None,
        callbacks: vec![Arc::new(|_: trogon_claude_agent_sdk::HookRequest| {
            Box::pin(async { Ok(HookOutput::default()) }) as trogon_claude_agent_sdk::HookFuture
        })],
        timeout: None,
        on_failure: None,
    }];
    let capture = run(options, &[]).await?;
    let entry = &capture.initialize["request"]["hooks"]["PreToolUse"][0];
    assert_eq!(entry["hookCallbackIds"], json!(["hook_0"]));
    assert!(entry.get("onFailure").is_none());
    Ok(())
}

#[tokio::test]
async fn overlay_environment_matches_python() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.environment_mode = EnvironmentMode::Overlay;
    options.cwd = Some("work".into());
    options.client_app = Some("my-app/1.2.3".into());
    options.trace_context = Some(TraceContext {
        traceparent: "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01".into(),
        tracestate: Some("k=v".into()),
    });
    options.env = Environment::from([("CLAUDE_AGENT_SDK_VERSION".into(), "forced-away".into())]);
    let capture = run(options, &[]).await?;
    assert_eq!(environment(&capture, "CLAUDECODE"), Some(None));
    assert_eq!(
        environment(&capture, "CLAUDE_CODE_ENTRYPOINT"),
        Some(Some("sdk-trogon-rs".into()))
    );
    assert_eq!(
        environment(&capture, "CLAUDE_AGENT_SDK_VERSION"),
        Some(Some(env!("CARGO_PKG_VERSION").into()))
    );
    assert_eq!(
        environment(&capture, "CLAUDE_AGENT_SDK_CLIENT_APP"),
        Some(Some("my-app/1.2.3".into()))
    );
    assert_eq!(environment(&capture, "CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP"), Some(None));
    assert_eq!(environment(&capture, "PWD"), Some(Some("work".into())));
    assert_eq!(environment(&capture, "TRACESTATE"), Some(Some("k=v".into())));
    assert_eq!(
        environment(&capture, "CLAUDE_CODE_SDK_READS_SESSION_STATE"),
        Some(Some("1".into()))
    );
    Ok(())
}

#[tokio::test]
async fn replace_environment_matches_typescript() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.environment_mode = EnvironmentMode::Replace;
    options.entrypoint = Some("custom-entry".into());
    options.env = Environment::from([("FOO".into(), "bar".into())]);
    let capture = run(options, &[]).await?;
    assert_eq!(environment(&capture, "FOO"), Some(Some("bar".into())));
    assert_eq!(
        environment(&capture, "CLAUDE_CODE_ENTRYPOINT"),
        Some(Some("custom-entry".into()))
    );
    assert_eq!(environment(&capture, "CLAUDECODE"), None);
    assert_eq!(environment(&capture, "PWD"), None);
    Ok(())
}

#[tokio::test]
async fn claudecode_marker_is_configurable() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.claude_code_marker = ClaudeCodeMarker::Keep;
    let capture = run(options, &[]).await?;
    assert_eq!(environment(&capture, "CLAUDECODE"), None);
    Ok(())
}

#[tokio::test]
async fn script_cli_runs_under_the_configured_runtime() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.cli_path = Some("cli.mjs".into());
    options.script_runtime = ScriptRuntime {
        executable: Some("bun".into()),
        arguments: vec!["--smol".into()],
    };
    let capture = run(options, &[]).await?;
    assert_eq!(capture.command.executable, "bun");
    assert_eq!(arguments(&capture)[..2], ["--smol", "cli.mjs"]);
    Ok(())
}

#[tokio::test]
async fn a_custom_spawner_skips_cli_discovery_like_upstream_custom_transports() -> Result<(), Box<dyn std::error::Error>>
{
    // No cli_path and an environment with an empty PATH: discovery would fail, but the caller owns spawning.
    let capture = run(ClaudeAgentOptions::default(), &[("PATH", ""), ("HOME", "/nonexistent")]).await?;
    assert_eq!(capture.command.executable, OsString::from("claude"));
    assert!(
        !arguments(&capture)
            .iter()
            .any(|argument| argument == "--await-initialize")
    );
    Ok(())
}

/// Neither SDK validates the budget, so every amount reaches argv in the selected SDK's spelling.
#[tokio::test]
async fn r03_budgets_reach_argv_unvalidated_in_each_sdk_spelling() -> Result<(), Box<dyn std::error::Error>> {
    for (amount, typescript, python) in [
        (0.0, "--max-budget-usd=0", "0.0"),
        (-1.0, "--max-budget-usd=-1", "-1.0"),
        (1e-7, "--max-budget-usd=1e-7", "1e-07"),
        (f64::NAN, "--max-budget-usd=NaN", "nan"),
        (f64::INFINITY, "--max-budget-usd=Infinity", "inf"),
    ] {
        let mut options = base();
        options.max_budget_usd = Some(UsdBudget::new(amount));
        let capture = run(options, &[]).await?;
        assert!(arguments(&capture).iter().any(|value| value == typescript), "{amount}");

        let mut options = base();
        options.argv_style = ArgvStyle::Python;
        options.max_budget_usd = Some(UsdBudget::new(amount));
        let arguments = arguments(&run(options, &[]).await?);
        let flag = arguments
            .iter()
            .position(|value| value == "--max-budget-usd")
            .ok_or("missing --max-budget-usd")?;
        assert_eq!(arguments.get(flag + 1).map(String::as_str), Some(python), "{amount}");
    }
    Ok(())
}

/// The servers both upstream probes were given, in the same insertion order: an `http` server with every
/// field and a caller key, a `stdio` server without `type`, an `sse` server whose URL is not a URL, an
/// `sdk` config without an instance, and one in-process server.
fn mcp_options(style: ArgvStyle) -> Result<ClaudeAgentOptions, Box<dyn std::error::Error>> {
    let mut options = base();
    options.argv_style = style;
    let mut tool = McpServerToolPolicy::new("search");
    tool.permission_policy = Some(McpToolPermissionPolicy::AlwaysAsk);
    let mut remote = McpRemoteServerOptions::new("https://example.com/mcp");
    remote.headers = Some(BTreeMap::from([("Authorization".into(), "Bearer fixture".into())]));
    remote.tools = Some(vec![tool]);
    remote.timeout = Some(5000);
    remote.always_load = Some(true);
    remote.extra = serde_json::from_value(json!({"oauth":{"clientId":"fixture"}}))?;
    options
        .mcp_servers
        .insert(McpServerName::from("remote"), McpServerOptions::Http(remote));
    options.mcp_servers.insert(
        McpServerName::from("local"),
        McpServerOptions::Stdio(McpStdioServerOptions {
            command: PathBuf::from("server"),
            args: Some(vec!["--flag".into(), "\u{e9}".into()]),
            env: Some(Environment::from([("K".into(), "v".into())])),
            ..McpStdioServerOptions::default()
        }),
    );
    options.mcp_servers.insert(
        McpServerName::from("events"),
        McpServerOptions::Sse(McpRemoteServerOptions::new("not a url")),
    );
    options.mcp_servers.insert(
        McpServerName::from("orphan"),
        McpServerOptions::Sdk(McpSdkServerOptions {
            name: "orphan".into(),
            ..McpSdkServerOptions::default()
        }),
    );
    let inproc: Arc<dyn SdkMcpServer> = Arc::new(LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from("inproc"),
        version: Some("1.0.0".into()),
        tools: Some(SdkTools::default()),
        instructions: None,
        always_load: false,
        dialect: McpServerDialect::TypeScript,
    }));
    options.sdk_mcp_servers.insert(McpServerName::from("inproc"), inproc);
    Ok(options)
}

fn mcp_config_values(capture: &Capture) -> Vec<String> {
    let arguments = arguments(capture);
    arguments
        .windows(2)
        .filter(|pair| pair[0] == "--mcp-config")
        .map(|pair| pair[1].clone())
        .collect()
}

/// Captured from `query()` in `sdk.mjs` 0.3.323 with a stand-in CLI: the in-process server is left out of
/// `--mcp-config` and named in `initialize` instead, and every other config is `JSON.stringify`'d as given.
#[tokio::test]
async fn typescript_mcp_config_matches_upstream_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let capture = run(mcp_options(ArgvStyle::TypeScript)?, &[]).await?;
    assert_eq!(
        mcp_config_values(&capture),
        [concat!(
            r#"{"mcpServers":{"remote":{"type":"http","url":"https://example.com/mcp","headers":{"Authorization":"Bearer fixture"},"#,
            r#""tools":[{"name":"search","permission_policy":"always_ask"}],"timeout":5000,"alwaysLoad":true,"oauth":{"clientId":"fixture"}},"#,
            r#""local":{"command":"server","args":["--flag","é"],"env":{"K":"v"}},"events":{"type":"sse","url":"not a url"},"#,
            r#""orphan":{"type":"sdk","name":"orphan"}}}"#
        )]
    );
    assert_eq!(capture.initialize["request"]["sdkMcpServers"], json!(["inproc"]));
    Ok(())
}

/// Captured from `SubprocessCLITransport._build_command` in Python 0.2.181: `json.dumps` spacing and ASCII
/// escapes, the in-process server listed as `{"type": "sdk", "name": ...}`, and nothing in `initialize`.
#[tokio::test]
async fn python_mcp_config_matches_upstream_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let capture = run(mcp_options(ArgvStyle::Python)?, &[]).await?;
    assert_eq!(
        mcp_config_values(&capture),
        [concat!(
            r#"{"mcpServers": {"remote": {"type": "http", "url": "https://example.com/mcp", "headers": {"Authorization": "Bearer fixture"}, "#,
            r#""tools": [{"name": "search", "permission_policy": "always_ask"}], "timeout": 5000, "alwaysLoad": true, "oauth": {"clientId": "fixture"}}, "#,
            r#""local": {"command": "server", "args": ["--flag", "\u00e9"], "env": {"K": "v"}}, "events": {"type": "sse", "url": "not a url"}, "#,
            r#""orphan": {"type": "sdk", "name": "orphan"}, "inproc": {"type": "sdk", "name": "inproc"}}}"#
        )]
    );
    assert!(capture.initialize["request"].get("sdkMcpServers").is_none());
    Ok(())
}

/// Python passes a `str` or `Path` `mcp_servers` straight through as the `--mcp-config` value.
#[tokio::test]
async fn python_mcp_config_path_is_passed_verbatim() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.argv_style = ArgvStyle::Python;
    options.mcp_config = Some("/srv/mcp.json".into());
    let capture = run(options, &[]).await?;
    assert_eq!(mcp_config_values(&capture), ["/srv/mcp.json"]);
    Ok(())
}

/// Python passes a `str` holding JSON text through as the `--mcp-config` value just like a path.
#[tokio::test]
async fn python_mcp_config_json_text_is_passed_verbatim() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.argv_style = ArgvStyle::Python;
    options.mcp_config = Some(r#"{"mcpServers":{}}"#.into());
    let capture = run(options, &[]).await?;
    assert_eq!(mcp_config_values(&capture), [r#"{"mcpServers":{}}"#]);
    Ok(())
}

/// TypeScript sends no `--mcp-config` when only in-process servers are configured.
#[tokio::test]
async fn typescript_in_process_servers_alone_send_no_mcp_config() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = mcp_options(ArgvStyle::TypeScript)?;
    options.mcp_servers.clear();
    let capture = run(options, &[]).await?;
    assert!(mcp_config_values(&capture).is_empty());
    assert_eq!(capture.initialize["request"]["sdkMcpServers"], json!(["inproc"]));
    Ok(())
}

/// A config path or JSON text given as `mcp_config` follows the built `--mcp-config` value in the TypeScript
/// argv too, as its own verbatim value.
#[tokio::test]
async fn typescript_mcp_config_path_is_passed_verbatim() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.argv_style = ArgvStyle::TypeScript;
    options.mcp_config = Some("/srv/mcp.json".into());
    let capture = run(options, &[]).await?;
    assert_eq!(mcp_config_values(&capture), ["/srv/mcp.json"]);
    Ok(())
}

/// TypeScript spreads the in-process servers over the configured ones, so an external server that shares an
/// in-process server's name is left out of `--mcp-config` and the name is sent in `initialize` once.
#[tokio::test]
async fn typescript_in_process_server_replaces_an_external_one_with_its_name() -> Result<(), Box<dyn std::error::Error>>
{
    let expected = mcp_config_values(&run(mcp_options(ArgvStyle::TypeScript)?, &[]).await?);
    let mut options = mcp_options(ArgvStyle::TypeScript)?;
    options.mcp_servers.insert(
        McpServerName::from("inproc"),
        McpServerOptions::Stdio(McpStdioServerOptions {
            command: PathBuf::from("shadowed"),
            ..McpStdioServerOptions::default()
        }),
    );
    let capture = run(options, &[]).await?;
    assert_eq!(mcp_config_values(&capture), expected);
    assert_eq!(capture.initialize["request"]["sdkMcpServers"], json!(["inproc"]));
    Ok(())
}
