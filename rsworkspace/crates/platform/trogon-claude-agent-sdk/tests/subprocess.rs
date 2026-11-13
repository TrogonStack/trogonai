#![cfg(feature = "test-support")]

use std::{error::Error as StdError, num::NonZeroUsize, sync::Arc, time::Duration};

use tokio::time::{sleep, timeout};
use trogon_claude_agent_sdk::{
    BuiltinToolName, ClaudeAgentOptions, ClaudeSdkClient, ContentBlock, ControlOperation, Error, FeatureSupport,
    FileCheckpointing, InMemorySessionStore, Message, MessageDecoding, PartialMessages, PermissionDecision, Prompt,
    QueueCapacity, SessionId, SessionStore, SessionStoreOptions, SystemPrompt, Timeout, ToolPermissionRequest,
    ToolRule, ToolSet, adapters::SystemClock, query,
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
                return Ok(result.result.unwrap_or_default());
            }
        }
        Err(std::io::Error::other("fixture ended without a result").into())
    })
    .await?
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
                results.push(result.result.unwrap_or_default());
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
                results.push(result.result.unwrap_or_default());
            }
        }
        Ok::<_, Error>(results)
    })
    .await??;
    assert_eq!(results, ["noisy"]);
    Ok(())
}

/// Both upstream SDKs end the run quietly when no idle state follows the last result before the run-end ceiling.
#[tokio::test]
async fn s05_a_missing_idle_state_ends_the_run_quietly_at_the_ceiling() -> TestResult {
    let mut options = options("stuck-running")?;
    options.limits.run_end_timeout = Timeout::new(Duration::from_millis(300));
    let mut response = query(Prompt::from("stuck"), options).await?;
    let results = timeout(Duration::from_secs(5), async {
        let mut results = Vec::new();
        while let Some(message) = response.next_message().await? {
            if let Message::Result(result) = message {
                results.push(result.result.unwrap_or_default());
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
    let info = client.connection_info();
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
    assert!(std::ptr::eq(info, client.connection_info()));
    client.send_prompt(Prompt::from("after metadata")).await?;
    assert_eq!(read_result(&mut client).await?, "after metadata");
    assert_eq!(serde_json::to_value(client.connection_info().as_json())?, raw);
    client.disconnect().await?;

    let client = ClaudeSdkClient::connect(options("normal")?).await?;
    assert_eq!(client.connection_info().hooks_applied, FeatureSupport::NotReported);
    assert_eq!(
        client.connection_info().hook_on_failure_supported,
        FeatureSupport::NotReported
    );
    assert_eq!(client.connection_info().as_json(), Some(&serde_json::json!({})));
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn interactive_client_reuses_one_process_for_multiple_turns() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("normal")?).await?;
    let process_id = client.process_id();
    client.send_prompt(Prompt::from("one")).await?;
    assert_eq!(read_result(&mut client).await?, "one");
    client.send_prompt(Prompt::from("two")).await?;
    assert_eq!(read_result(&mut client).await?, "two");
    assert_eq!(client.process_id(), process_id);
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn argv_binds_values_and_forwards_cwd() -> TestResult {
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
    Ok(())
}

#[tokio::test]
async fn permission_requests_default_to_deny() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("permission")?).await?;
    client.send_prompt(Prompt::from("permission")).await?;
    assert_eq!(read_result(&mut client).await?, "denied");
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
async fn cancelled_permission_callback_cannot_reply_later() -> TestResult {
    let mut options = options("permission-cancel")?;
    options.permission_handler = Some(Arc::new(|_: ToolPermissionRequest| async {
        sleep(Duration::from_millis(100)).await;
        PermissionDecision::allow()
    }));
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("cancel")).await?;
    assert_eq!(read_result(&mut client).await?, "cancelled-permission");
    sleep(Duration::from_millis(150)).await;
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
    options.limits.max_buffer_size = Some(NonZeroUsize::new(1024).ok_or_else(|| std::io::Error::other("zero limit"))?);
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
        assert!(client.connection_info().models.is_empty());
        assert_eq!(client.connection_info().as_json(), Some(&raw));
        client.disconnect().await?;
    }
    Ok(())
}

#[tokio::test]
async fn successful_process_exit_without_a_result_is_not_success() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("no-result")?).await?;
    client.send_prompt(Prompt::from("missing result")).await?;
    let error = loop {
        match client.next_message().await {
            Ok(Some(_)) => {}
            Err(error) => break error,
            Ok(None) => return Err(std::io::Error::other("missing terminal error").into()),
        }
    };
    assert!(matches!(error, Error::UnexpectedEof));
    client.disconnect().await?;
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

#[cfg(unix)]
#[tokio::test]
async fn initialization_timeout_reaps_process() -> TestResult {
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join("pid");
    let mut options = options("init-timeout")?;
    options.system_prompt = SystemPrompt::Custom(marker.to_string_lossy().into_owned());
    options.limits.initialize_timeout =
        trogon_claude_agent_sdk::InitializeTimeout::Fixed(Timeout::new(Duration::from_millis(100)));
    assert!(matches!(
        ClaudeSdkClient::connect(options).await,
        Err(Error::ControlTimeout {
            operation: ControlOperation::Initialize
        })
    ));
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
                    values.push(message.tool_use_result);
                }
                Message::Result(_) => return Ok::<_, Error>(values),
                _ => {}
            }
        }
        Err(Error::UnexpectedEof)
    })
    .await;
    let cleanup = client.disconnect().await;
    let values = observed??;
    cleanup?;
    Ok(values)
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
    let events = observed??;
    cleanup?;
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
