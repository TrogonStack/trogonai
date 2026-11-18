#![cfg(feature = "test-support")]

use std::{error::Error as StdError, ffi::OsStr, sync::Arc, time::Duration};

use tokio::time::{error::Elapsed, sleep, timeout};
use trogon_claude_agent_sdk::{
    BuiltinToolName, ClaudeAgentOptions, ClaudeSdkClient, ContentBlock, ControlOperation, EndWithoutResult, Error,
    FeatureSupport, FileCheckpointing, InMemorySessionStore, Message, MessageDecoding, PartialMessages,
    PermissionDecision, Prompt, QueueCapacity, RunEndTimeout, SessionId, SessionStore, SessionStoreOptions,
    StderrDelivery, StderrHandler, SystemPrompt, Timeout, ToolPermissionRequest, ToolRule, ToolSet,
    adapters::SystemClock, query,
};

type TestResult = Result<(), Box<dyn StdError>>;

fn options(mode: &str) -> Result<ClaudeAgentOptions, Box<dyn StdError>> {
    let mut options = ClaudeAgentOptions {
        cli_path: Some(std::path::absolute(env!("CARGO_BIN_EXE_claude-sdk-fixture"))?),
        model: Some(format!("fixture:{mode}").into()),
        ..ClaudeAgentOptions::default()
    };
    options.limits.control_timeout = Some(Timeout::new(Duration::from_secs(2)));
    options.limits.shutdown = trogon_claude_agent_sdk::Shutdown::Strict(Timeout::new(Duration::from_millis(200)));
    // The runtime force-kills the fixture on shutdown, possibly while it is writing its coverage
    // profile at exit, which leaves a corrupt profile. The fixture runs no crate code, so its
    // profile is discarded.
    options.env.insert("LLVM_PROFILE_FILE".into(), "/dev/null".into());
    Ok(options)
}

async fn read_result(client: &mut ClaudeSdkClient) -> Result<String, Box<dyn StdError>> {
    timeout(Duration::from_secs(5), async {
        while let Some(message) = client.next_message().await? {
            if let Message::Result(result) = message {
                return Ok(result.result.clone().unwrap_or_default());
            }
        }
        Err(std::io::Error::other("fixture ended without a result").into())
    })
    .await?
}

/// Keeps a paused clock still for as long as it lives: Tokio does not auto-advance a paused clock while a blocking
/// task runs, so no SDK deadline fires before the fixture event it guards, however slowly the fixture starts or
/// exits. Only the test lets the clock go, by ending the hold or dropping it on an early return, so no wall-clock
/// bound can release it while a loaded machine is still starting the fixture.
struct ClockHold {
    release: std::sync::mpsc::Sender<()>,
    hold: tokio::task::JoinHandle<()>,
}

impl ClockHold {
    fn start() -> Self {
        let (release, held) = std::sync::mpsc::channel::<()>();
        // Nothing is ever sent: `recv` returns once the sender is gone.
        let hold = tokio::task::spawn_blocking(move || {
            let _ = held.recv();
        });
        Self { release, hold }
    }

    /// Lets the clock go, and returns once it may auto-advance again.
    async fn end(self) -> TestResult {
        drop(self.release);
        self.hold.await?;
        Ok(())
    }
}

#[tokio::test]
async fn query_streams_typed_messages_and_closes() -> TestResult {
    let mut query = query(Prompt::from("first turn"), options("normal")?).await?;
    let mut saw_assistant = false;
    let mut saw_result = false;
    while let Some(message) = query.next_message().await? {
        match message {
            Message::Assistant(message) => {
                saw_assistant = true;
                assert!(matches!(&message.content[0], ContentBlock::Text { text } if text == "first turn"));
            }
            Message::Result(result) => {
                saw_result = true;
                assert_eq!(result.result.as_deref(), Some("first turn"));
            }
            _ => {}
        }
    }
    assert!(saw_assistant && saw_result);
    Ok(())
}

#[tokio::test]
async fn query_preserves_background_continuation_until_idle() -> TestResult {
    let mut response = query(Prompt::from("background"), options("full-run")?).await?;
    let results = timeout(Duration::from_secs(3), async {
        let mut results = Vec::new();
        while let Some(message) = response.next_message().await? {
            if let Message::Result(result) = message {
                results.push(result.result.clone().unwrap_or_default());
            }
        }
        Ok::<_, Error>(results)
    })
    .await??;
    assert_eq!(results, ["first-result", "continuation-result"]);
    Ok(())
}

/// Both upstream SDKs skip stdout lines that are not JSON objects, so CLI banners and blank lines never end a session.
#[tokio::test]
async fn s02_non_json_stdout_lines_are_skipped() -> TestResult {
    let mut response = query(Prompt::from("noisy"), options("noisy-stdout")?).await?;
    let results = timeout(Duration::from_secs(5), async {
        let mut results = Vec::new();
        while let Some(message) = response.next_message().await? {
            if let Message::Result(result) = message {
                results.push(result.result.clone().unwrap_or_default());
            }
        }
        Ok::<_, Error>(results)
    })
    .await??;
    assert_eq!(results, ["noisy"]);
    Ok(())
}

/// Both upstream SDKs end the run quietly when no idle state follows the last result before the run-end ceiling.
/// The ceiling only applies while a permission callback keeps the CLI talking; without any bidirectional need both
/// SDKs close the input at once and the run ends when the CLI exits.
#[tokio::test]
async fn s05_a_missing_idle_state_ends_the_run_quietly_at_the_ceiling() -> TestResult {
    let mut options = options("stuck-running")?;
    options.permission_handler = Some(Arc::new(|_: ToolPermissionRequest| async {
        std::future::pending::<PermissionDecision>().await
    }));
    options.limits.run_end_timeout = RunEndTimeout::Fixed(Timeout::new(Duration::from_millis(300)));
    let mut response = query(Prompt::from("stuck"), options).await?;
    let results = timeout(Duration::from_secs(5), async {
        let mut results = Vec::new();
        while let Some(message) = response.next_message().await? {
            if let Message::Result(result) = message {
                results.push(result.result.clone().unwrap_or_default());
            }
        }
        Ok::<_, Error>(results)
    })
    .await??;
    assert_eq!(results, ["stuck"]);
    Ok(())
}

/// Both upstream SDKs refuse file checkpointing together with a session store.
#[tokio::test]
async fn w2_file_checkpointing_with_a_session_store_is_refused() -> TestResult {
    let mut options = options("normal")?;
    options.file_checkpointing = FileCheckpointing::Enabled;
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::<SystemClock>::default());
    options.session_store = Some(SessionStoreOptions::from(store));
    let Err(error @ Error::FileCheckpointingWithSessionStore) = ClaudeSdkClient::connect(options).await else {
        return Err("file checkpointing with a session store was not refused".into());
    };
    assert_eq!(
        error.to_string(),
        "enableFileCheckpointing is not yet supported with sessionStore (backup blobs are not mirrored, so rewindFiles() fails after a store-backed resume)."
    );
    assert_eq!(
        error.python_message(),
        Some(
            "session_store cannot be combined with enable_file_checkpointing (checkpoints are local-disk only and would diverge from the mirrored transcript)"
        )
    );
    Ok(())
}

/// Both upstream SDKs refuse `continue` against a store that cannot list sessions.
#[tokio::test]
async fn st06_continue_conversation_requires_a_store_that_lists_sessions() -> TestResult {
    struct RequiredOnly;

    impl SessionStore for RequiredOnly {
        fn append<'a>(
            &'a self,
            _key: &'a trogon_claude_agent_sdk::SessionKey,
            _entries: &'a [trogon_claude_agent_sdk::SessionEntry],
        ) -> trogon_claude_agent_sdk::SessionStoreFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }

        fn load<'a>(
            &'a self,
            _key: &'a trogon_claude_agent_sdk::SessionKey,
        ) -> trogon_claude_agent_sdk::SessionStoreFuture<'a, Option<Vec<trogon_claude_agent_sdk::SessionEntry>>>
        {
            Box::pin(async { Ok(None) })
        }
    }

    let mut options = options("normal")?;
    options.continue_conversation = trogon_claude_agent_sdk::ContinueConversation::Yes;
    let store: Arc<dyn SessionStore> = Arc::new(RequiredOnly);
    options.session_store = Some(SessionStoreOptions::from(store));
    let Err(error @ Error::ContinueRequiresListSessions) = ClaudeSdkClient::connect(options).await else {
        return Err("continue against a store that cannot list sessions was not refused".into());
    };
    assert_eq!(
        error.to_string(),
        "Options.continue with sessionStore requires store.listSessions to be implemented"
    );
    assert_eq!(
        error.python_message(),
        Some("continue_conversation with session_store requires the store to implement list_sessions()")
    );
    assert_eq!(Error::InputClosed.python_message(), None);
    Ok(())
}

#[tokio::test]
async fn known_message_metadata_survives_decoding() -> TestResult {
    let mut response = query(Prompt::from("metadata"), options("metadata")?).await?;
    let mut saw_assistant = false;
    let mut saw_result = false;
    while let Some(message) = response.next_message().await? {
        match message {
            Message::Assistant(message) => {
                saw_assistant = true;
                assert_eq!(message.uuid.as_ref().map(|id| id.as_str()), Some("assistant-envelope"));
                assert_eq!(message.message_id.as_ref().map(|id| id.as_str()), Some("model-message"));
                assert_eq!(message.stop_reason.as_deref(), Some("end_turn"));
                assert_eq!(message.additional_fields["future_outer"], 1);
                assert_eq!(message.message_fields["future_body"], 2);
            }
            Message::Result(message) => {
                saw_result = true;
                assert_eq!(message.uuid.as_ref().map(|id| id.as_str()), Some("result-envelope"));
                assert_eq!(
                    message.origin.as_ref().map(|origin| origin.kind()),
                    Some("task-notification")
                );
                assert_eq!(message.terminal_reason.as_deref(), Some("completed"));
                assert_eq!(
                    message
                        .model_usage
                        .as_ref()
                        .and_then(|usage| usage.get("fixture-model"))
                        .and_then(|model| model.get("inputTokens")),
                    Some(&serde_json::json!(2))
                );
                assert_eq!(message.additional_fields["future_result"], 3);
                assert_eq!(message.result_index, Some(1));
            }
            _ => {}
        }
    }
    assert!(saw_assistant && saw_result);
    Ok(())
}

#[tokio::test]
async fn connected_client_exposes_cached_lossless_metadata() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("connection-info")?).await?;
    let info = client.handle().initialization_result().await?;
    assert_eq!(info.commands[0]["name"], "review");
    assert_eq!(info.models[0]["value"], "fixture-model");
    assert_eq!(info.agents[0]["name"], "reviewer");
    assert_eq!(
        info.account.as_ref().and_then(|account| account.get("organization")),
        Some(&serde_json::json!("fixture"))
    );
    assert_eq!(info.hooks_applied, FeatureSupport::NotReported);
    assert_eq!(info.hook_on_failure_supported, FeatureSupport::Supported);
    assert_eq!(info.claude_code_version.as_deref(), Some("2.1.319"));
    assert_eq!(
        info.additional_fields["future_metadata"],
        serde_json::json!({"nested":[1,null]})
    );
    let raw = serde_json::json!({
        "commands":[{"name":"review"}],
        "models":[{"value":"fixture-model"}],
        "agents":[{"name":"reviewer"}],
        "account":{"organization":"fixture"},
        "hooks_applied":null,
        "hook_on_failure_supported":true,
        "claude_code_version":"2.1.319",
        "future_metadata":{"nested":[1,null]},
    });
    assert_eq!(serde_json::to_value(info.as_json())?, raw);
    assert_eq!(client.handle().initialization_result().await?.as_json(), info.as_json());
    client.send_prompt(Prompt::from("after metadata")).await?;
    assert_eq!(read_result(&mut client).await?, "after metadata");
    assert_eq!(
        serde_json::to_value(client.handle().initialization_result().await?.as_json())?,
        raw
    );
    client.disconnect().await?;

    let client = ClaudeSdkClient::connect(options("normal")?).await?;
    let info = client.handle().initialization_result().await?;
    assert_eq!(info.hooks_applied, FeatureSupport::NotReported);
    assert_eq!(info.hook_on_failure_supported, FeatureSupport::NotReported);
    assert_eq!(info.as_json(), Some(&serde_json::json!({})));
    client.disconnect().await?;
    Ok(())
}

/// Python `ClaudeSDKClient(options).connect(prompt)`: the native connector spawns the CLI, and the prompt is written
/// once the connection is up.
#[tokio::test]
async fn an_unconnected_client_spawns_the_cli_and_writes_its_prompt() -> TestResult {
    let mut client = ClaudeSdkClient::new(options("normal")?).connect("hello").await?;
    assert_eq!(read_result(&mut client).await?, "hello");
    client.disconnect().await?;
    Ok(())
}

/// The turns, the exit after stdin closes and the stderr close are fixture events; the clock stays still so the
/// result wait and the shutdown deadlines cannot run out before them on a loaded runner.
#[tokio::test(start_paused = true)]
async fn interactive_client_reuses_one_process_for_multiple_turns() -> TestResult {
    let clock = ClockHold::start();
    let mut client = ClaudeSdkClient::connect(options("normal")?).await?;
    let process_id = client.process_id();
    client.send_prompt(Prompt::from("one")).await?;
    assert_eq!(read_result(&mut client).await?, "one");
    client.send_prompt(Prompt::from("two")).await?;
    assert_eq!(read_result(&mut client).await?, "two");
    assert_eq!(client.process_id(), process_id);
    client.disconnect().await?;
    clock.end().await
}

/// Python's `_find_cli` takes `<package>/_bundled/claude` before anything else; `bundled_cli_root` names that
/// directory, and the executable it holds is the one `connect` spawns.
#[cfg(unix)]
#[tokio::test]
async fn connect_spawns_the_bundled_cli_under_the_configured_root() -> TestResult {
    let root = tempfile::tempdir()?;
    let mut options = options("normal")?;
    let fixture = options.cli_path.take().ok_or("the fixture has a path")?;
    std::fs::create_dir(root.path().join("_bundled"))?;
    std::os::unix::fs::symlink(fixture, root.path().join("_bundled/claude"))?;
    options.bundled_cli_root = Some(root.path().to_path_buf());
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("bundled")).await?;
    assert_eq!(read_result(&mut client).await?, "bundled");
    client.disconnect().await?;
    Ok(())
}

/// The init frame, the exit after stdin closes and the stderr close are fixture events; the clock stays still so
/// the shutdown deadlines cannot run out before them on a loaded runner.
#[tokio::test(start_paused = true)]
async fn argv_binds_values_and_forwards_cwd() -> TestResult {
    let clock = ClockHold::start();
    let directory = tempfile::tempdir()?;
    let mut options = options("argv")?;
    options.setting_sources = Some(Vec::new());
    options.cwd = Some(directory.path().to_path_buf());
    options.resume = Some(SessionId::from("--permission-mode=bypassPermissions"));
    options.allowed_tools = vec![ToolRule::from("Bash(git diff *)")];
    options.tools = ToolSet::Only(vec![BuiltinToolName::from("Read")]);
    options.disallowed_tools = vec![ToolRule::from("Bash")];
    let mut client = ClaudeSdkClient::connect(options).await?;
    let Some(Message::System(init)) = client.next_message().await? else {
        return Err(std::io::Error::other("missing init").into());
    };
    let arguments = init.data["arguments"]
        .as_array()
        .ok_or_else(|| std::io::Error::other("missing arguments"))?;
    assert!(
        arguments
            .iter()
            .any(|arg| arg == "--resume=--permission-mode=bypassPermissions")
    );
    assert!(!arguments.iter().any(|arg| arg == "--permission-mode=bypassPermissions"));
    assert!(arguments.iter().any(|arg| arg == "--allowedTools=Bash(git diff *)"));
    assert!(arguments.iter().any(|arg| arg == "--tools=Read"));
    assert!(arguments.iter().any(|arg| arg == "--disallowedTools=Bash"));
    assert!(arguments.iter().any(|arg| arg == "--setting-sources="));
    assert_eq!(
        std::path::Path::new(
            init.data["cwd"]
                .as_str()
                .ok_or_else(|| std::io::Error::other("missing cwd"))?
        )
        .canonicalize()?,
        directory.path().canonicalize()?
    );
    client.disconnect().await?;
    clock.end().await
}

#[tokio::test]
async fn permission_requests_without_a_handler_get_the_typescript_error() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("permission")?).await?;
    client.send_prompt(Prompt::from("permission")).await?;
    assert_eq!(read_result(&mut client).await?, "canUseTool callback is not provided.");
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn permission_callback_can_replace_tool_input() -> TestResult {
    let mut options = options("permission")?;
    options.permission_handler = Some(Arc::new(|request: ToolPermissionRequest| async move {
        assert_eq!(request.tool_name.as_str(), "Bash");
        let mut input = request.input;
        input.insert("command".into(), "printf approved".into());
        PermissionDecision::Allow {
            updated_input: Some(input),
            updated_permissions: None,
            decision_classification: None,
        }
    }));
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("permission")).await?;
    assert_eq!(read_result(&mut client).await?, "printf approved");
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn unknown_control_request_is_rejected() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("unknown-control")?).await?;
    client.send_prompt(Prompt::from("unknown")).await?;
    assert_eq!(read_result(&mut client).await?, "unsupported-control-rejected");
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn interrupt_completes_without_consuming_messages() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("interrupt")?).await?;
    client.send_prompt(Prompt::from("wait")).await?;
    client.handle().interrupt().await?;
    assert_eq!(read_result(&mut client).await?, "interrupted");
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn interrupt_progresses_under_continuous_control_output() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("continuous")?).await?;
    client.send_prompt(Prompt::from("flood")).await?;
    sleep(Duration::from_millis(50)).await;
    timeout(Duration::from_secs(2), client.handle().interrupt()).await??;
    assert_eq!(read_result(&mut client).await?, "interrupted");
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn pending_callback_does_not_block_interrupt() -> TestResult {
    let mut options = options("callback-pending")?;
    options.permission_handler = Some(Arc::new(|_: ToolPermissionRequest| async {
        std::future::pending::<PermissionDecision>().await
    }));
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("wait")).await?;
    client.handle().interrupt().await?;
    assert_eq!(read_result(&mut client).await?, "interrupted");
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn a_cancelled_permission_callback_still_answers_like_the_typescript_sdk() -> TestResult {
    let mut options = options("permission-cancel")?;
    options.permission_handler = Some(Arc::new(|request: ToolPermissionRequest| async move {
        request.signal.cancelled().await;
        sleep(Duration::from_millis(50)).await;
        PermissionDecision::allow()
    }));
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("cancel")).await?;
    assert_eq!(read_result(&mut client).await?, "allow");
    client.handle().interrupt().await?;
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn partial_events_are_typed() -> TestResult {
    let mut options = options("stream")?;
    options.partial_messages = PartialMessages::Include;
    let mut query = query(Prompt::from("stream"), options).await?;
    let mut streamed = false;
    while let Some(message) = query.next_message().await? {
        if let Message::StreamEvent(event) = message {
            streamed = true;
            assert_eq!(event.event["delta"]["text"], "streamed");
        }
    }
    assert!(streamed);
    Ok(())
}

#[tokio::test]
async fn cancelling_a_receive_preserves_partial_frame() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("partial")?).await?;
    let _ = client.next_message().await?;
    client.send_prompt(Prompt::from("after cancellation")).await?;
    assert!(timeout(Duration::from_millis(20), client.next_message()).await.is_err());
    assert_eq!(read_result(&mut client).await?, "after cancellation");
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn malformed_known_frame_is_a_terminal_protocol_error_with_python_decoding() -> TestResult {
    let mut options = options("malformed")?;
    options.message_decoding = MessageDecoding::Python;
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("malformed")).await?;
    let error = loop {
        match client.next_message().await {
            Ok(Some(_)) => {}
            Err(error) => break error,
            Ok(None) => return Err(std::io::Error::other("missing protocol error").into()),
        }
    };
    assert!(matches!(error, Error::MessageParse(_)));
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn oversized_frame_is_rejected() -> TestResult {
    let mut options = options("oversized")?;
    options.argv_style = trogon_claude_agent_sdk::ArgvStyle::Python;
    options.limits.max_buffer_size = Some(1024);
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("oversized")).await?;
    let error = loop {
        match client.next_message().await {
            Ok(Some(_)) => {}
            Err(error) => break error,
            Ok(None) => return Err(std::io::Error::other("missing error").into()),
        }
    };
    let Error::JsonDecode(decode) = error else {
        return Err(std::io::Error::other("expected the buffer guard").into());
    };
    assert_eq!(decode.line(), "JSON message exceeded maximum buffer size of 1024 bytes");
    assert!(matches!(
        decode.original_error(),
        trogon_claude_agent_sdk::JsonDecodeSourceError::BufferExceeded { limit: 1024, .. }
    ));
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn a_bad_frame_stops_a_cli_that_is_still_running() -> TestResult {
    let mut options = options("oversized-live")?;
    options.argv_style = trogon_claude_agent_sdk::ArgvStyle::Python;
    options.limits.max_buffer_size = Some(1024);
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("oversized")).await?;
    // The fixture sleeps for 30 seconds after the bad frame, so ending well before that means the SDK stopped it.
    let error = timeout(Duration::from_secs(10), async {
        loop {
            match client.next_message().await {
                Ok(Some(_)) => {}
                Err(error) => return Ok(error),
                Ok(None) => return Err(std::io::Error::other("missing error")),
            }
        }
    })
    .await??;
    assert!(matches!(error, Error::JsonDecode(_)), "{error:?}");
    timeout(Duration::from_secs(10), client.disconnect()).await??;
    Ok(())
}

#[tokio::test]
async fn process_failure_preserves_status_and_bounded_stderr() -> TestResult {
    let mut options = options("failure")?;
    options.limits.stderr_tail = 32;
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("failure")).await?;
    let error = loop {
        match client.next_message().await {
            Ok(Some(_)) => {}
            Err(error) => break error,
            Ok(None) => return Err(std::io::Error::other("missing error").into()),
        }
    };
    let Error::Process { status, stderr } = error else {
        return Err(std::io::Error::other("expected process error").into());
    };
    assert_eq!(status.code(), Some(7));
    assert_eq!(stderr.as_bytes().len(), 32);
    assert!(stderr.as_bytes().ends_with(b"diagnostic-tail"));
    assert!(!format!("{stderr:?}").contains("diagnostic-tail"));
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn process_failure_message_redacts_secrets_from_the_stderr_tail() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("failure-secret")?).await?;
    client.send_prompt(Prompt::from("failure")).await?;
    let error = loop {
        match client.next_message().await {
            Ok(Some(_)) => {}
            Err(error) => break error,
            Ok(None) => return Err(std::io::Error::other("missing error").into()),
        }
    };
    assert_eq!(
        error.typescript_message().as_deref(),
        Some("Claude Code process exited with code 7. stderr: rejected key <token> at startup")
    );
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn python_keeps_the_stderr_tail_it_opts_into_unredacted() -> TestResult {
    let mut options = options("failure-secret")?;
    options.argv_style = trogon_claude_agent_sdk::ArgvStyle::Python;
    options.limits.stderr_tail = 1024;
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("failure")).await?;
    let error = loop {
        match client.next_message().await {
            Ok(Some(_)) => {}
            Err(error) => break error,
            Ok(None) => return Err(std::io::Error::other("missing error").into()),
        }
    };
    let Error::Process { status, stderr } = error else {
        return Err(std::io::Error::other("expected process error").into());
    };
    assert_eq!(status.code(), Some(7));
    let secret = format!("sk-{}", "Ab1".repeat(8));
    assert_eq!(
        stderr.as_bytes(),
        format!("rejected key {secret} at startup").as_bytes()
    );
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn consumer_lag_fails_with_a_bounded_queue() -> TestResult {
    let mut options = options("queue-overflow")?;
    options.limits.message_buffer = trogon_claude_agent_sdk::MessageBuffer::FailWhenFull(QueueCapacity::new(1)?);
    let mut client = ClaudeSdkClient::connect(options).await?;
    let _ = client.next_message().await?;
    client.send_prompt(Prompt::from("flood")).await?;
    // The control reply follows the fixture's flood, without consuming the bounded queue. When the overflow stops the
    // run before the interrupt is written, the interrupt fails like any write to a closed transport.
    let fence = timeout(Duration::from_secs(5), client.handle().interrupt()).await?;
    assert!(
        matches!(&fence, Ok(_) | Err(Error::DriverStopped | Error::NotReadyForWriting)),
        "unexpected producer fence outcome: {fence:?}"
    );
    let Some(Message::System(first)) = client.next_message().await? else {
        return Err(std::io::Error::other("missing retained queue event").into());
    };
    assert_eq!(first.subtype, "fixture-event");
    assert_eq!(first.data["index"], 0);
    let observed = client.next_message().await;
    assert!(
        matches!(&observed, Err(Error::MessageQueueFull)),
        "unexpected bounded-queue outcome: {observed:?}"
    );
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn missing_control_reply_times_out() -> TestResult {
    let mut options = options("control-timeout")?;
    options.limits.control_timeout = Some(Timeout::new(Duration::from_millis(100)));
    let client = ClaudeSdkClient::connect(options).await?;
    assert!(matches!(
        client.handle().interrupt().await,
        Err(Error::ControlTimeout {
            operation: ControlOperation::Interrupt
        })
    ));
    assert!(matches!(
        client.disconnect().await,
        Err(Error::ControlTimeout {
            operation: ControlOperation::Interrupt
        })
    ));
    Ok(())
}

#[tokio::test]
async fn malformed_initialization_is_accepted_like_upstream() -> TestResult {
    for (mode, raw) in [
        ("init-malformed", serde_json::json!(42)),
        ("init-malformed-known", serde_json::json!({"models":"invalid"})),
    ] {
        let client = ClaudeSdkClient::connect(options(mode)?).await?;
        let info = client.handle().initialization_result().await?;
        assert!(info.models.is_empty());
        assert_eq!(info.as_json(), Some(&raw));
        client.disconnect().await?;
    }
    Ok(())
}

/// Reads every message after a prompt the CLI exits on with status 0 and never answers with a result.
async fn read_without_a_result(options: ClaudeAgentOptions) -> Result<Result<(), Error>, Box<dyn StdError>> {
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("missing result")).await?;
    let ended = loop {
        match client.next_message().await {
            Ok(Some(_)) => {}
            Ok(None) => break Ok(()),
            Err(error) => break Err(error),
        }
    };
    client.disconnect().await?;
    Ok(ended)
}

/// TypeScript and Python treat only a failed exit as an error, so a clean exit in the middle of a turn ends the
/// messages; `EndWithoutResult::Fail` keeps `UnexpectedEof` as an opt-in.
#[tokio::test]
async fn a_clean_exit_without_a_result_ends_the_messages_unless_the_limit_fails_it() -> TestResult {
    let ended = read_without_a_result(options("no-result")?).await?;
    assert!(ended.is_ok(), "{ended:?}");

    let mut failing = options("no-result")?;
    failing.limits.end_without_result = EndWithoutResult::Fail;
    let ended = read_without_a_result(failing).await?;
    assert!(matches!(ended, Err(Error::UnexpectedEof)), "{ended:?}");
    Ok(())
}

#[tokio::test]
async fn queue_capacity_rejects_values_that_would_panic() -> TestResult {
    let mut options = options("normal")?;
    match usize::MAX.try_into() {
        Ok(capacity) => options.limits.message_buffer = trogon_claude_agent_sdk::MessageBuffer::FailWhenFull(capacity),
        Err(_) => return Ok(()),
    }
    let outcome = tokio::spawn(ClaudeSdkClient::connect(options)).await;
    assert!(
        matches!(outcome, Ok(Err(_))),
        "invalid queue capacity panicked inside the SDK"
    );
    Ok(())
}

#[tokio::test]
async fn timeout_rejects_values_that_would_overflow_an_instant() -> TestResult {
    let mut options = options("normal")?;
    options.limits.control_timeout = Some(Timeout::new(Duration::MAX));
    let outcome = ClaudeSdkClient::connect(options).await;
    assert!(
        !matches!(outcome, Err(Error::DriverStopped)),
        "invalid timeout panicked inside the driver"
    );
    Ok(())
}

async fn tool_arguments(input: &str) -> Result<Vec<serde_json::Value>, Box<dyn StdError>> {
    let mut options = options("argv")?;
    options.setting_sources = Some(Vec::new());
    options.tools = ToolSet::Only(vec![BuiltinToolName::from(input)]);
    let mut client = ClaudeSdkClient::connect(options).await?;
    let Some(Message::System(init)) = client.next_message().await? else {
        return Err(std::io::Error::other("missing init").into());
    };
    let arguments = init.data["arguments"]
        .as_array()
        .ok_or_else(|| std::io::Error::other("missing arguments"))?
        .clone();
    client.disconnect().await?;
    Ok(arguments)
}

/// Both upstream SDKs join tool names with commas and pass them to the CLI without validating each name.
#[tokio::test]
async fn s14_tool_names_pass_through_to_the_cli_unvalidated() -> TestResult {
    for input in ["Read,Bash", "default"] {
        let expected = serde_json::Value::from(format!("--tools={input}"));
        assert!(tool_arguments(input).await?.contains(&expected));
    }
    Ok(())
}

#[cfg(unix)]
async fn assert_reaped(pid: u32) -> TestResult {
    timeout(Duration::from_secs(3), async {
        loop {
            let status = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?;
            if !status.success() {
                return Ok::<(), Box<dyn StdError>>(());
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn dropped_owner_reaps_process_even_when_handle_survives() -> TestResult {
    let client = ClaudeSdkClient::connect(options("normal")?).await?;
    let pid = client
        .process_id()
        .ok_or_else(|| std::io::Error::other("missing pid"))?;
    let handle = client.handle();
    drop(client);
    assert_reaped(pid).await?;
    // TypeScript: the exit event clears `ready`, so a later write is not ready.
    assert!(matches!(handle.interrupt().await, Err(Error::NotReadyForWriting)));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn python_style_reports_the_terminated_process_for_a_handle_that_outlived_its_owner() -> TestResult {
    let mut python = options("normal")?;
    python.argv_style = trogon_claude_agent_sdk::ArgvStyle::Python;
    let client = ClaudeSdkClient::connect(python).await?;
    let pid = client
        .process_id()
        .ok_or_else(|| std::io::Error::other("missing pid"))?;
    let handle = client.handle();
    drop(client);
    assert_reaped(pid).await?;
    assert!(matches!(
        handle.interrupt().await,
        Err(Error::WriteToTerminatedProcess { .. })
    ));
    Ok(())
}

/// The initialize timeout starts when `connect` sends initialize, and the fixture writes its pid only after it reads
/// that request, so a slow start could time out before the marker exists. The clock stays still until the fixture
/// says the marker is written, then moves by exactly the timeout; the kill and the exit are events from there.
#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn initialization_timeout_reaps_process() -> TestResult {
    const INITIALIZE_TIMEOUT: Duration = Duration::from_millis(100);
    let clock = ClockHold::start();
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join("pid");
    let mut options = options("init-timeout")?;
    options.system_prompt = SystemPrompt::Custom(marker.to_string_lossy().into_owned());
    options.limits.initialize_timeout =
        trogon_claude_agent_sdk::InitializeTimeout::Fixed(Timeout::new(INITIALIZE_TIMEOUT));
    let (stderr, mut lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    options.stderr_handler = Some(StderrHandler {
        delivery: StderrDelivery::Lines,
        callback: Arc::new(move |line: &str| {
            let _ = stderr.send(line.to_owned());
        }),
    });
    // TypeScript hands the client out before initialize is answered, so the timeout rejects the initialization
    // result and ends the run.
    let client = ClaudeSdkClient::connect(options).await?;
    timeout(Duration::from_secs(3_600), async {
        while let Some(line) = lines.recv().await {
            if line == "pid marker written" {
                return Ok(());
            }
        }
        Err(std::io::Error::other("the fixture ended before writing its pid"))
    })
    .await??;
    tokio::time::advance(INITIALIZE_TIMEOUT).await;
    let rejected = client.handle().initialization_result().await;
    assert!(matches!(
        &rejected,
        Err(Error::InitializationFailed(inner))
            if matches!(**inner, Error::ControlTimeout { operation: ControlOperation::Initialize })
    ));
    assert!(matches!(
        client.disconnect().await,
        Err(Error::ControlTimeout {
            operation: ControlOperation::Initialize
        })
    ));
    clock.end().await?;
    // The reap check polls on the wall clock, as it did before the hold.
    tokio::time::resume();
    let pid: u32 = std::fs::read_to_string(marker)?.parse()?;
    assert_reaped(pid).await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_connect_reaps_the_started_process() -> TestResult {
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join("pid");
    let mut options = options("init-timeout")?;
    options.system_prompt = SystemPrompt::Custom(marker.to_string_lossy().into_owned());
    // Only Python waits for the initialize response inside `connect`, so only there is there something to cancel.
    options.argv_style = trogon_claude_agent_sdk::ArgvStyle::Python;
    let connect = tokio::spawn(ClaudeSdkClient::connect(options));
    timeout(Duration::from_secs(3), async {
        while !marker.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    let pid: u32 = std::fs::read_to_string(marker)?.parse()?;
    connect.abort();
    assert!(connect.await.is_err());
    assert_reaped(pid).await?;
    Ok(())
}

/// Reading the run and the disconnect after it both failed; each error is kept for the report.
#[derive(Debug, thiserror::Error)]
#[error("reading the run failed: {read}; disconnecting failed: {disconnect}")]
struct ReadAndDisconnectError {
    #[source]
    read: Box<dyn StdError>,
    disconnect: Error,
}

/// What the run read, after the disconnect that follows it; when both fail, the report carries both errors.
fn read_then_disconnect<T>(
    observed: Result<Result<T, Error>, Elapsed>,
    cleanup: Result<(), Error>,
) -> Result<T, Box<dyn StdError>> {
    let observed = match observed {
        Ok(read) => read.map_err(Box::<dyn StdError>::from),
        Err(elapsed) => Err(elapsed.into()),
    };
    match (observed, cleanup) {
        (Err(read), Err(disconnect)) => Err(ReadAndDisconnectError { read, disconnect }.into()),
        (observed, cleanup) => {
            let value = observed?;
            cleanup?;
            Ok(value)
        }
    }
}

async fn compatibility_tool_result_frames(mode: &str) -> Result<Vec<Option<serde_json::Value>>, Box<dyn StdError>> {
    let mut client = ClaudeSdkClient::connect(options(mode)?).await?;
    client.send_prompt(Prompt::from("tool output")).await?;
    let observed = timeout(Duration::from_secs(5), async {
        let mut values = Vec::new();
        while let Some(message) = client.next_message().await? {
            match message {
                Message::User(message) => {
                    assert_eq!(message.additional_fields["future_outer"], 1);
                    assert_eq!(message.message_fields["future_body"], 2);
                    assert!(!message.additional_fields.contains_key("tool_use_result"));
                    values.push(message.tool_use_result.clone());
                }
                Message::Result(_) => return Ok::<_, Error>(values),
                _ => {}
            }
        }
        Err(Error::UnexpectedEof)
    })
    .await;
    let cleanup = client.disconnect().await;
    read_then_disconnect(observed, cleanup)
}

#[tokio::test]
async fn compatibility_user_tool_result_distinguishes_omitted_and_explicit_null() -> TestResult {
    assert_eq!(
        compatibility_tool_result_frames("tool-result-null").await?,
        vec![None, Some(serde_json::Value::Null)]
    );
    Ok(())
}

#[tokio::test]
async fn compatibility_user_tool_result_retains_all_json_kinds() -> TestResult {
    assert_eq!(
        compatibility_tool_result_frames("tool-result-values").await?,
        vec![
            None,
            Some(serde_json::json!(true)),
            Some(serde_json::json!(42)),
            Some(serde_json::json!("result")),
            Some(serde_json::json!([null, true, {"nested": []}])),
            Some(serde_json::json!({"nested": [null, false]})),
            Some(serde_json::Value::Null),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn compatibility_stream_event_retains_outer_metadata_separately() -> TestResult {
    let mut settings = options("stream-metadata")?;
    settings.partial_messages = PartialMessages::Include;
    let mut client = ClaudeSdkClient::connect(settings).await?;
    client.send_prompt(Prompt::from("stream")).await?;
    let observed = timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        while let Some(message) = client.next_message().await? {
            match message {
                Message::StreamEvent(event) => events.push(event),
                Message::Result(_) => return Ok::<_, Error>(events),
                _ => {}
            }
        }
        Err(Error::UnexpectedEof)
    })
    .await;
    let cleanup = client.disconnect().await;
    let events = read_then_disconnect(observed, cleanup)?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].uuid, "event-id");
    assert_eq!(events[0].session_id.as_str(), "fixture-session");
    assert!(events[0].parent_tool_use_id.is_none());
    assert_eq!(
        serde_json::to_value(&events[0].additional_fields)?,
        serde_json::json!({
            "ttft_ms":12.5,"user_message_uuid":"user-one","user_message_uuids":["user-one","user-two"],"resume_reason":"future-reason","future":{"outside":[null,true]}
        })
    );
    assert_eq!(
        serde_json::to_value(&events[0].event)?,
        serde_json::json!({
            "type":"content_block_delta","delta":{"type":"text_delta","text":"streamed"},"future":{"inside":null}
        })
    );
    Ok(())
}

type Recorded<T> = Arc<std::sync::Mutex<Vec<T>>>;

fn recorded<T: Clone>(slot: &Recorded<T>) -> Result<Vec<T>, Box<dyn StdError>> {
    Ok(slot.lock().map_err(|_| std::io::Error::other("poisoned"))?.clone())
}

/// A `spawn_claude_code_process` that records what it was given and runs it as the SDK would.
fn recording_spawner(
    seen: Recorded<trogon_claude_agent_sdk::SpawnOptions>,
) -> trogon_claude_agent_sdk::SpawnClaudeCodeProcess {
    trogon_claude_agent_sdk::SpawnClaudeCodeProcess(Arc::new(move |spawn: trogon_claude_agent_sdk::SpawnOptions| {
        let mut command = tokio::process::Command::new(&spawn.command);
        command.args(&spawn.args).env_clear().envs(&spawn.env);
        if let Some(cwd) = &spawn.cwd {
            command.current_dir(cwd);
        }
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if let Ok(mut seen) = seen.lock() {
            seen.push(spawn);
        }
        command.spawn()
    }))
}

fn spawn_reporter(calls: Recorded<(u32, f64)>) -> trogon_claude_agent_sdk::OnProcessSpawned {
    trogon_claude_agent_sdk::OnProcessSpawned(Arc::new(
        move |pid: u32, info: trogon_claude_agent_sdk::ProcessSpawnInfo| {
            if let Ok(mut calls) = calls.lock() {
                calls.push((pid, info.spawn_sync_ms));
            }
        },
    ))
}

#[tokio::test]
async fn a_custom_spawner_gets_the_full_command_and_its_process_runs_the_query() -> TestResult {
    let directory = tempfile::tempdir()?;
    let seen = Recorded::default();
    let calls = Recorded::default();
    let mut options = options("normal")?;
    options.cwd = Some(directory.path().to_path_buf());
    options.env.insert("SPAWN_MARKER".into(), "custom".into());
    options.spawn_claude_code_process = Some(recording_spawner(seen.clone()));
    options.on_process_spawned = Some(spawn_reporter(calls.clone()));
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("through the spawner")).await?;
    assert_eq!(read_result(&mut client).await?, "through the spawner");
    client.disconnect().await?;
    let seen = recorded(&seen)?;
    let [spawn] = seen.as_slice() else {
        return Err(std::io::Error::other(format!("expected one spawn, got {}", seen.len())).into());
    };
    assert_eq!(
        std::path::Path::new(&spawn.command),
        std::path::absolute(env!("CARGO_BIN_EXE_claude-sdk-fixture"))?
    );
    assert!(
        spawn
            .args
            .iter()
            .any(|arg| arg.to_string_lossy().contains("fixture:normal"))
    );
    // TypeScript derives no default debug file for a custom spawner.
    assert!(
        !spawn
            .args
            .iter()
            .any(|arg| arg.to_string_lossy().starts_with("--debug-file"))
    );
    assert_eq!(spawn.cwd.as_deref(), Some(directory.path()));
    assert_eq!(
        spawn.env.get(OsStr::new("SPAWN_MARKER")).map(|value| value.as_os_str()),
        Some(OsStr::new("custom"))
    );
    // The spawner gets the whole child environment, not only the SDK's changes. TypeScript builds it from
    // `options.env` alone when that is given (`env ? {...env} : {...process.env}`), so the caller's entries arrive
    // and the inherited `PATH` does not.
    assert_eq!(
        spawn
            .env
            .get(OsStr::new("LLVM_PROFILE_FILE"))
            .map(|value| value.as_os_str()),
        Some(OsStr::new("/dev/null"))
    );
    assert_eq!(spawn.env.get(OsStr::new("PATH")), None);
    assert_eq!(spawn.uid, None);
    // Closing without an abort leaves the forwarded signal alone, and TypeScript reports no spawn it did not make.
    assert!(!spawn.signal.is_cancelled());
    assert!(recorded(&calls)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_failing_custom_spawner_is_a_spawn_error() -> TestResult {
    let mut options = options("normal")?;
    options.spawn_claude_code_process = Some(trogon_claude_agent_sdk::SpawnClaudeCodeProcess(Arc::new(
        |_: trogon_claude_agent_sdk::SpawnOptions| -> std::io::Result<tokio::process::Child> {
            Err(std::io::Error::from(std::io::ErrorKind::HostUnreachable))
        },
    )));
    let error = ClaudeSdkClient::connect(options)
        .await
        .err()
        .ok_or_else(|| std::io::Error::other("connect succeeded without a process"))?;
    assert!(
        matches!(&error, Error::Spawn(source) if source.kind() == std::io::ErrorKind::HostUnreachable),
        "{error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn the_forwarded_signal_is_cancelled_only_once_an_aborted_client_has_closed() -> TestResult {
    let seen = Recorded::default();
    let controller = trogon_claude_agent_sdk::CancellationToken::new();
    let mut options = options("blocked-stdin")?;
    options.abort_controller = Some(controller.clone());
    options.spawn_claude_code_process = Some(recording_spawner(seen.clone()));
    let mut client = ClaudeSdkClient::connect(options).await?;
    let signal = recorded(&seen)?
        .first()
        .map(|spawn| spawn.signal.clone())
        .ok_or_else(|| std::io::Error::other("the spawner was not called"))?;
    controller.cancel();
    // The forwarded signal is not the caller's: it waits for the close.
    assert!(!signal.is_cancelled());
    let end = timeout(Duration::from_secs(10), async {
        loop {
            match client.next_message().await {
                Ok(Some(_)) => {}
                Ok(None) => return None,
                Err(error) => return Some(error),
            }
        }
    })
    .await?;
    assert!(matches!(end, Some(Error::Aborted(_))), "{end:?}");
    let _ = client.disconnect().await;
    assert!(signal.is_cancelled());
    Ok(())
}

#[tokio::test]
async fn on_process_spawned_reports_the_pid_of_the_sdk_spawn() -> TestResult {
    let calls = Recorded::default();
    let mut options = options("normal")?;
    options.on_process_spawned = Some(spawn_reporter(calls.clone()));
    let client = ClaudeSdkClient::connect(options).await?;
    let process_id = client.process_id();
    client.disconnect().await?;
    let calls = recorded(&calls)?;
    let [(pid, spawn_sync_ms)] = calls.as_slice() else {
        return Err(std::io::Error::other(format!("expected one report, got {}", calls.len())).into());
    };
    assert_eq!(Some(*pid), process_id);
    assert!(spawn_sync_ms.is_finite() && *spawn_sync_ms >= 0.0);
    Ok(())
}

#[tokio::test]
async fn a_panicking_on_process_spawned_does_not_stop_the_query() -> TestResult {
    let mut options = options("normal")?;
    options.on_process_spawned = Some(trogon_claude_agent_sdk::OnProcessSpawned(Arc::new(
        |pid: u32, _: trogon_claude_agent_sdk::ProcessSpawnInfo| {
            if pid > 0 {
                std::panic::resume_unwind(Box::new("scripted onProcessSpawned failure"));
            }
        },
    )));
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("still runs")).await?;
    assert_eq!(read_result(&mut client).await?, "still runs");
    client.disconnect().await?;
    Ok(())
}
