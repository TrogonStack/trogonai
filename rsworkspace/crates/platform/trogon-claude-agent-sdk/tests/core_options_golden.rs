#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests answer the initialize request through a scripted child process."
    )
)]

//! Pins the core option argv, environment, stderr and error behavior against goldens captured from the real
//! `sdk.mjs` 0.3.323 and `claude-agent-sdk` 0.2.181 packages driving a recording stand-in for the CLI.

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    ArgvStyle, ClaudeAgentOptions, ClaudeSdkClient, ContinueConversation, Error, ExtraArgument, InMemorySessionStore,
    PartialMessages, SessionFork, SessionPersistence, SessionStore, SessionStoreOptions, StderrDelivery, StderrHandler,
    ToolSet,
    adapters::{NativeFileSystem, RuntimeAdapters, SystemClock},
    test_support::FixedEnvironment,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;

use scripted_process::{CapturedProcessCommand, ScriptedProcessHarness, ScriptedProcessSpawner};

type TestResult = Result<(), Box<dyn std::error::Error>>;
/// (case, client_app option, value in options.env, value inherited from the parent process)
type ClientAppCase = (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

const GOLDENS: &str = include_str!("fixtures/core_options/upstream_options.json");

async fn serve(process: ScriptedProcessHarness, stderr_chunks: Vec<Vec<u8>>) -> Result<(), std::io::Error> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let mut stderr = process.stderr;
    while let Some(line) = lines.next_line().await? {
        let request: Value = serde_json::from_str(&line)?;
        if request["request"]["subtype"] == "initialize" {
            for chunk in &stderr_chunks {
                stderr.write_all(chunk).await?;
                stderr.flush().await?;
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
            let mut frame = serde_json::to_vec(&json!({"type":"control_response","response":{
                "subtype":"success","request_id":request["request_id"],
                "response":{"hooks_applied":true,"hook_on_failure_supported":true,"claude_code_version":"2.1.319"}}}))?;
            frame.push(b'\n');
            stdout.write_all(&frame).await?;
        }
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok(())
}

async fn run_with(
    options: ClaudeAgentOptions,
    variables: &[(&str, &str)],
    stderr_chunks: Vec<Vec<u8>>,
) -> Result<CapturedProcessCommand, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let server = tokio::spawn(serve(harness, stderr_chunks));
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
    server.await??;
    Ok(observer.commands()?.remove(0))
}

async fn argv(options: ClaudeAgentOptions) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let command = run_with(options, &[], Vec::new()).await?;
    Ok(command
        .arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect())
}

fn base(style: ArgvStyle) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        argv_style: style,
        ..ClaudeAgentOptions::default()
    }
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

fn directories(items: &[&str]) -> Vec<PathBuf> {
    items.iter().map(PathBuf::from).collect()
}

fn rules(items: &[&str]) -> Vec<trogon_claude_agent_sdk::ToolRule> {
    items.iter().map(|item| (*item).into()).collect()
}

fn tools(items: &[&str]) -> ToolSet {
    ToolSet::Only(items.iter().map(|item| (*item).into()).collect())
}

type Case = (&'static str, fn(&mut ClaudeAgentOptions));

fn expected(goldens: &Value, dialect: &str, name: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let tokens = goldens[dialect][name]
        .as_array()
        .ok_or_else(|| format!("no golden for {dialect} {name}"))?;
    Ok(tokens
        .iter()
        .map(|token| token.as_str().unwrap_or_default().to_owned())
        .collect())
}

#[tokio::test]
async fn typescript_core_options_match_the_upstream_argv() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let prefix = argv(base(ArgvStyle::TypeScript)).await?;
    let cases: Vec<Case> = vec![
        ("additional_directories", |o| {
            o.additional_directories = directories(&["/a", "-b", "", "/c d"]);
        }),
        ("additional_directories_empty", |_| {}),
        ("agent_named", |o| o.agent = Some("reviewer".into())),
        ("agent_empty", |o| o.agent = Some("".into())),
        ("allowed_tools", |o| {
            o.allowed_tools = rules(&["Read", "Bash(ls:*)", "mcp__s__t"]);
        }),
        ("allowed_tools_empty", |_| {}),
        ("allowed_tools_empty_item", |o| o.allowed_tools = rules(&[""])),
        ("disallowed_tools", |o| {
            o.disallowed_tools = rules(&["Write", "Bash(rm:*)"]);
        }),
        ("disallowed_tools_empty", |_| {}),
        ("continue_true", |o| o.continue_conversation = ContinueConversation::Yes),
        ("continue_false", |_| {}),
        ("resume_id", |o| {
            o.resume = Some("11111111-1111-4111-8111-111111111111".into());
        }),
        ("resume_empty", |o| o.resume = Some("".into())),
        ("resume_dash", |o| o.resume = Some("-x".into())),
        ("fork_session", |o| o.fork_session = SessionFork::Fork),
        ("fork_session_false", |_| {}),
        ("partial_messages", |o| o.partial_messages = PartialMessages::Include),
        ("partial_messages_false", |_| {}),
        ("max_turns", |o| o.max_turns = NonZeroUsize::new(7)),
        ("max_turns_zero", |o| o.max_turns = NonZeroUsize::new(0)),
        ("persist_false", |o| {
            o.session_persistence = SessionPersistence::Disabled
        }),
        ("persist_true", |_| {}),
        ("tools_list", |o| o.tools = tools(&["Read", "Edit"])),
        ("tools_empty", |o| o.tools = tools(&[])),
        ("tools_preset", |o| o.tools = ToolSet::ClaudeCodePreset),
        ("extra_args_mixed", |o| {
            o.extra_args = extras(&[
                ("bare-flag", None),
                ("spaced", Some("value")),
                ("dashed", Some("-1")),
                ("dash_only", Some("-")),
                ("empty", Some("")),
                ("two_dash", Some("--x")),
                ("a b", Some("c d")),
                ("9", Some("nine")),
                ("zeta", Some("z")),
                ("1", Some("one")),
            ]);
        }),
        ("extra_args_empty", |_| {}),
        ("extra_args_order_all", |o| {
            o.partial_messages = PartialMessages::Include;
            o.fork_session = SessionFork::Fork;
            o.session_persistence = SessionPersistence::Disabled;
            o.max_turns = NonZeroUsize::new(2);
            o.additional_directories = directories(&["/d"]);
            o.extra_args = extras(&[("zz", None)]);
            o.allowed_tools = rules(&["Read"]);
            o.disallowed_tools = rules(&["Bash"]);
            o.tools = tools(&["Read"]);
            o.agent = Some("a".into());
            o.continue_conversation = ContinueConversation::Yes;
            o.resume = Some("r1".into());
        }),
        ("extra_args_sdk_url_guard", |o| {
            o.resume = Some("s".into());
            o.extra_args = extras(&[("sdk-url", Some("x"))]);
        }),
    ];
    for (name, apply) in cases {
        let mut options = base(ArgvStyle::TypeScript);
        apply(&mut options);
        let mut want = prefix.clone();
        want.extend(expected(&goldens, "typescript", name)?);
        assert_eq!(argv(options).await?, want, "typescript case {name}");
    }
    Ok(())
}

#[tokio::test]
async fn python_core_options_match_the_upstream_argv() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let full = argv(base(ArgvStyle::Python)).await?;
    let split = full.len() - 2;
    let cases: Vec<Case> = vec![
        ("add_dirs", |o| {
            o.additional_directories = directories(&["/a", "-b", "", "/c d"]);
        }),
        ("add_dirs_empty", |_| {}),
        ("allowed", |o| o.allowed_tools = rules(&["Read", "Bash(ls:*)"])),
        ("allowed_empty", |_| {}),
        ("disallowed", |o| o.disallowed_tools = rules(&["Write"])),
        ("continue_true", |o| o.continue_conversation = ContinueConversation::Yes),
        ("continue_false", |_| {}),
        ("resume", |o| {
            o.resume = Some("11111111-1111-4111-8111-111111111111".into());
        }),
        ("resume_empty", |o| o.resume = Some("".into())),
        ("resume_dash", |o| o.resume = Some("-x".into())),
        ("fork", |o| o.fork_session = SessionFork::Fork),
        ("partial", |o| o.partial_messages = PartialMessages::Include),
        ("max_turns", |o| o.max_turns = NonZeroUsize::new(7)),
        ("max_turns_zero", |o| o.max_turns = NonZeroUsize::new(0)),
        ("tools_list", |o| o.tools = tools(&["Read", "Edit"])),
        ("tools_empty", |o| o.tools = tools(&[])),
        ("tools_preset", |o| o.tools = ToolSet::ClaudeCodePreset),
        ("extra_mixed", |o| {
            o.extra_args = extras(&[
                ("bare-flag", None),
                ("spaced", Some("value")),
                ("dashed", Some("-1")),
                ("dash_only", Some("-")),
                ("empty", Some("")),
                ("two_dash", Some("--x")),
                ("a b", Some("c d")),
                ("9", Some("nine")),
            ]);
        }),
        ("extra_order_all", |o| {
            o.partial_messages = PartialMessages::Include;
            o.fork_session = SessionFork::Fork;
            o.max_turns = NonZeroUsize::new(2);
            o.additional_directories = directories(&["/d"]);
            o.extra_args = extras(&[("zz", None)]);
            o.allowed_tools = rules(&["Read"]);
            o.disallowed_tools = rules(&["Bash"]);
            o.tools = tools(&["Read"]);
            o.continue_conversation = ContinueConversation::Yes;
            o.resume = Some("r1".into());
        }),
    ];
    for (name, apply) in cases {
        let mut options = base(ArgvStyle::Python);
        apply(&mut options);
        let mut want = full[..split].to_vec();
        want.extend(expected(&goldens, "python", name)?);
        want.extend(full[split..].iter().cloned());
        assert_eq!(argv(options).await?, want, "python case {name}");
    }
    Ok(())
}

#[tokio::test]
async fn python_client_app_follows_the_upstream_precedence() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let key = "CLAUDE_AGENT_SDK_CLIENT_APP";
    let cases: [ClientAppCase; 6] = [
        ("client_app_x", Some("x"), None, None),
        ("client_app_empty", Some(""), None, None),
        ("client_app_none", None, None, None),
        ("client_app_opt_env", Some("x"), Some("fromenv"), None),
        ("client_app_inherited", None, None, Some("inh")),
        ("client_app_inherited_plus_opt", Some("x"), None, Some("inh")),
    ];
    for (name, option, configured, inherited) in cases {
        let mut options = base(ArgvStyle::Python);
        options.client_app = option.map(str::to_owned);
        if let Some(value) = configured {
            options.env.insert(key.into(), value.into());
        }
        let variables: Vec<(&str, &str)> = inherited.map(|value| (key, value)).into_iter().collect();
        let command = run_with(options, &variables, Vec::new()).await?;
        let set = command
            .environment
            .get(&OsString::from(key))
            .and_then(|value| value.as_ref())
            .map(|value| value.to_string_lossy().into_owned());
        let effective = set.or_else(|| inherited.map(str::to_owned));
        let want = goldens["python_client_app"][name].as_str().map(str::to_owned);
        assert_eq!(effective, want, "client app case {name}");
    }
    Ok(())
}

async fn stderr_seen(
    style: ArgvStyle,
    delivery: StderrDelivery,
    chunks: Vec<Vec<u8>>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&seen);
    let mut options = base(style);
    options.stderr_handler = Some(StderrHandler {
        delivery,
        callback: Arc::new(move |text: &str| {
            if let Ok(mut lines) = sink.lock() {
                lines.push(text.to_owned());
            }
        }),
    });
    run_with(options, &[], chunks).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    Ok(seen.lock().map(|lines| lines.clone()).unwrap_or_default())
}

fn text_chunks() -> Vec<Vec<u8>> {
    ["one\n", "two  \n\nthree", " tail\n", "x\u{e9}", "end"]
        .iter()
        .map(|chunk| chunk.as_bytes().to_vec())
        .collect()
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

#[tokio::test]
async fn typescript_stderr_reaches_the_callback_as_raw_chunks() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let seen = stderr_seen(ArgvStyle::TypeScript, StderrDelivery::Chunks, text_chunks()).await?;
    // Pipe reads have no record boundaries, so only the delivered text is deterministic.
    assert_eq!(seen.concat(), strings(&goldens["typescript_stderr_chunks"]).concat());
    Ok(())
}

#[tokio::test]
async fn python_stderr_reaches_the_callback_as_trimmed_lines() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let seen = stderr_seen(ArgvStyle::Python, StderrDelivery::Lines, text_chunks()).await?;
    assert_eq!(seen, strings(&goldens["python_stderr_lines"]));
    Ok(())
}

/// Node's `StringDecoder` holds the first byte of a split character until the rest arrives.
#[tokio::test]
async fn typescript_stderr_holds_a_split_character_across_chunks() -> TestResult {
    let chunks = vec![vec![0x61, 0xc3], vec![0xa9, 0x62, 0x0a]];
    let seen = stderr_seen(ArgvStyle::TypeScript, StderrDelivery::Chunks, chunks).await?;
    assert_eq!(seen.concat(), "a\u{e9}b\n");
    assert!(seen.iter().all(|chunk| !chunk.contains('\u{fffd}')), "{seen:?}");
    Ok(())
}

#[tokio::test]
async fn typescript_refuses_a_session_store_without_persistence() -> TestResult {
    let mut options = base(ArgvStyle::TypeScript);
    options.session_persistence = SessionPersistence::Disabled;
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::<SystemClock>::default());
    options.session_store = Some(SessionStoreOptions::from(store));
    let error = run_with(options, &[], Vec::new())
        .await
        .err()
        .ok_or("connect succeeded")?;
    let error = error.downcast::<Error>().map_err(|_| "not an sdk error")?;
    assert!(matches!(*error, Error::SessionStoreWithoutPersistence), "{error:?}");
    Ok(())
}
