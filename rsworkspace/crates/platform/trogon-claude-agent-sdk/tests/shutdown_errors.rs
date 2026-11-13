#![cfg(feature = "test-support")]

use std::{error::Error as StdError, path::PathBuf, time::Duration};

use tokio::time::timeout;
use trogon_claude_agent_sdk::{
    ArgvStyle, ClaudeAgentOptions, ClaudeSdkClient, Error, Message, MessageDecoding, Prompt, Timeout,
};

type TestResult = Result<(), Box<dyn StdError>>;

fn options(mode: &str, style: ArgvStyle) -> Result<ClaudeAgentOptions, Box<dyn StdError>> {
    let mut options = ClaudeAgentOptions {
        cli_path: Some(std::path::absolute(env!("CARGO_BIN_EXE_claude-sdk-fixture"))?),
        model: Some(format!("fixture:{mode}").into()),
        argv_style: style,
        ..ClaudeAgentOptions::default()
    };
    options.limits.control_timeout = Timeout::new(Duration::from_secs(2))?;
    options.limits.shutdown_timeout = Timeout::new(Duration::from_millis(500))?;
    options.env.insert("LLVM_PROFILE_FILE".into(), "/dev/null".into());
    Ok(options)
}

/// Drives one prompt to the first error, or to the end of the stream when there is none.
async fn terminal_error(mode: &str, style: ArgvStyle) -> Result<(Vec<Message>, Option<Error>), Box<dyn StdError>> {
    terminal_error_with(options(mode, style)?).await
}

async fn terminal_error_with(options: ClaudeAgentOptions) -> Result<(Vec<Message>, Option<Error>), Box<dyn StdError>> {
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("go")).await?;
    client.finish_input().await?;
    let collected = timeout(Duration::from_secs(10), async {
        let mut messages = Vec::new();
        loop {
            match client.next_message().await {
                Ok(Some(message)) => messages.push(message),
                Ok(None) => return (messages, None),
                Err(error) => return (messages, Some(error)),
            }
        }
    })
    .await?;
    let _ = client.disconnect().await;
    Ok(collected)
}

#[tokio::test]
async fn t04_process_error_display_is_the_python_text_and_typescript_text_is_available() -> TestResult {
    let (_, error) = terminal_error("failure", ArgvStyle::Python).await?;
    let error = error.ok_or_else(|| std::io::Error::other("missing error"))?;
    assert_eq!(
        error.to_string(),
        "Command failed with exit code 7 (exit code: 7)\nError output: Check stderr output for details"
    );
    assert_eq!(error.exit_code(), Some(7));
    let typescript = error
        .typescript_message()
        .ok_or_else(|| std::io::Error::other("no typescript text"))?;
    assert!(typescript.starts_with("Claude Code process exited with code 7. stderr: "));
    assert!(typescript.ends_with("diagnostic-tail"));
    Ok(())
}

#[tokio::test]
async fn e05_q13_python_replaces_the_exit_error_with_the_result_error() -> TestResult {
    let (_, error) = terminal_error("error-result-exit", ArgvStyle::Python).await?;
    let Some(Error::Result(result)) = error else {
        return Err(std::io::Error::other("expected a result error").into());
    };
    assert_eq!(result.message(), "Claude Code returned an error result: boom; second");
    assert_eq!(result.exit_code(), Some(1));
    assert_eq!(result.subtype(), Some("error_during_execution"));
    assert_eq!(result.errors(), ["boom", "second"]);
    assert_eq!(result.api_error_status(), Some(529));
    assert_eq!(result.session_id(), Some("fixture-session"));
    assert_eq!(result.data().get("is_error"), Some(&serde_json::Value::Bool(true)));
    Ok(())
}

#[tokio::test]
async fn e05_python_error_result_without_text_falls_back_to_the_subtype() -> TestResult {
    let (_, error) = terminal_error("error-result-blank-exit", ArgvStyle::Python).await?;
    let Some(Error::Result(result)) = error else {
        return Err(std::io::Error::other("expected a result error").into());
    };
    assert_eq!(
        result.message(),
        "Claude Code returned an error result: error_during_execution"
    );
    Ok(())
}

#[tokio::test]
async fn e05_typescript_uses_errors_and_keeps_the_process_error_when_the_text_is_blank() -> TestResult {
    let (_, error) = terminal_error("error-result-exit", ArgvStyle::TypeScript).await?;
    let Some(Error::Result(result)) = error else {
        return Err(std::io::Error::other("expected a result error").into());
    };
    assert_eq!(result.message(), "Claude Code returned an error result: boom; second");

    let (_, error) = terminal_error("error-result-success-subtype-exit", ArgvStyle::TypeScript).await?;
    let Some(Error::Result(result)) = error else {
        return Err(std::io::Error::other("expected a result error").into());
    };
    assert_eq!(result.message(), "Claude Code returned an error result: rate limited");

    let (_, error) = terminal_error("error-result-blank-exit", ArgvStyle::TypeScript).await?;
    assert!(matches!(error, Some(Error::Process { .. })));
    Ok(())
}

#[tokio::test]
async fn t03_typescript_skips_a_corrupt_line_and_python_fails_on_it() -> TestResult {
    let (messages, error) = terminal_error("corrupt-line", ArgvStyle::TypeScript).await?;
    assert!(error.is_none());
    assert!(messages.iter().any(|message| matches!(message, Message::Result(_))));

    let (_, error) = terminal_error("corrupt-line", ArgvStyle::Python).await?;
    assert!(matches!(error, Some(Error::JsonDecode(_))), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn e03_json_decode_error_carries_the_line_and_the_original_error() -> TestResult {
    let (_, error) = terminal_error("corrupt-line", ArgvStyle::Python).await?;
    let Some(Error::JsonDecode(decode)) = error else {
        return Err(std::io::Error::other("expected a JSON decode error").into());
    };
    assert_eq!(decode.line(), "{\"type\": oops");
    assert!(
        matches!(decode.original_error(), trogon_claude_agent_sdk::JsonDecodeSourceError::Json(error) if error.is_syntax())
    );
    assert_eq!(
        Error::from(decode).to_string(),
        "Failed to decode JSON: {\"type\": oops..."
    );
    Ok(())
}

#[tokio::test]
async fn e04_message_parse_error_carries_the_raw_data() -> TestResult {
    for style in [ArgvStyle::Python, ArgvStyle::TypeScript] {
        let mut python = options("malformed", style)?;
        python.message_decoding = MessageDecoding::Python;
        let (_, error) = terminal_error_with(python).await?;
        let Some(Error::MessageParse(parse)) = error else {
            return Err(
                std::io::Error::other(format!("{style:?}: expected a message parse error, got {error:?}")).into(),
            );
        };
        let data = parse.data().ok_or_else(|| std::io::Error::other("missing data"))?;
        assert_eq!(data.get("type"), Some(&serde_json::json!("assistant")));
        assert_eq!(parse.message(), "Missing required field in assistant message: 'model'");
    }
    Ok(())
}

#[tokio::test]
async fn typescript_decoding_yields_a_known_message_it_cannot_type() -> TestResult {
    for style in [ArgvStyle::Python, ArgvStyle::TypeScript] {
        let (messages, error) = terminal_error("malformed", style).await?;
        assert!(!matches!(error, Some(Error::MessageParse(_))), "{style:?}: {error:?}");
        assert!(
            messages
                .iter()
                .any(|message| matches!(message, Message::Unknown { kind, .. } if kind == "assistant")),
            "{style:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn t14_a_write_after_stdin_closed_is_not_ready_or_terminated() -> TestResult {
    let client = ClaudeSdkClient::connect(options("normal", ArgvStyle::Python)?).await?;
    client.send_prompt(Prompt::from("go")).await?;
    client.finish_input().await?;
    // Once the result settled the run, stdin is closed; the CLI then exits cleanly and the run stops.
    let error = timeout(Duration::from_secs(10), async {
        loop {
            match client.send_prompt(Prompt::from("late")).await {
                Err(Error::InputClosed) | Ok(()) => tokio::time::sleep(Duration::from_millis(20)).await,
                Err(error) => return error,
            }
        }
    })
    .await?;
    match error {
        Error::NotReadyForWriting => assert_eq!(error.to_string(), "ProcessTransport is not ready for writing"),
        Error::WriteToTerminatedProcess { exit_code: Some(0) } => {
            assert_eq!(error.to_string(), "Cannot write to terminated process (exit code: 0)");
            assert_eq!(
                error.typescript_message().as_deref(),
                Some("Cannot write to terminated process")
            );
        }
        other => return Err(std::io::Error::other(format!("unexpected: {other:?}")).into()),
    }
    let _ = client.disconnect().await;
    Ok(())
}

#[tokio::test]
async fn t14_a_write_after_abort_is_the_operation_aborted_error() -> TestResult {
    let client = ClaudeSdkClient::connect(options("blocked-stdin", ArgvStyle::TypeScript)?).await?;
    let handle = client.handle();
    client.abort().await?;
    let error = timeout(Duration::from_secs(10), async {
        loop {
            match handle.send_prompt(Prompt::from("late")).await {
                Err(Error::Aborted(aborted)) => return aborted,
                _ => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await?;
    assert_eq!(error.message(), "Operation aborted");
    let _ = client.disconnect().await;
    Ok(())
}

#[tokio::test]
async fn t14_a_write_after_the_cli_exited_with_an_error_reports_the_upstream_state() -> TestResult {
    for style in [ArgvStyle::Python, ArgvStyle::TypeScript] {
        let mut client = ClaudeSdkClient::connect(options("failure", style)?).await?;
        client.send_prompt(Prompt::from("go")).await?;
        let first = timeout(Duration::from_secs(10), async {
            loop {
                match client.next_message().await {
                    Ok(Some(_)) => {}
                    Ok(None) => return None,
                    Err(error) => return Some(error),
                }
            }
        })
        .await?;
        assert!(matches!(first, Some(Error::Process { .. })), "{style:?}: {first:?}");
        let error = client
            .send_prompt(Prompt::from("again"))
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("write after exit succeeded"))?;
        // Upstream checks "not ready" and "terminated" before "exited with error", and neither SDK can reach that
        // branch after a real exit: Python keeps `_ready` and reports the exit code, TypeScript's exit event clears
        // `ready`.
        match style {
            ArgvStyle::Python => {
                assert!(
                    matches!(error, Error::WriteToTerminatedProcess { exit_code: Some(7) }),
                    "{error:?}"
                );
                assert_eq!(error.to_string(), "Cannot write to terminated process (exit code: 7)");
            }
            _ => {
                assert!(matches!(error, Error::NotReadyForWriting), "{error:?}");
                assert_eq!(error.to_string(), "ProcessTransport is not ready for writing");
            }
        }
        let _ = client.disconnect().await;
    }
    Ok(())
}

#[tokio::test]
async fn t03_a_corrupt_end_of_stream_tail_is_dropped_by_both_styles() -> TestResult {
    for style in [ArgvStyle::TypeScript, ArgvStyle::Python] {
        let (messages, error) = terminal_error("corrupt-tail", style).await?;
        assert!(error.is_none(), "{style:?}: {error:?}");
        assert!(messages.iter().any(|message| matches!(message, Message::Result(_))));
    }
    Ok(())
}

#[tokio::test]
async fn t10_a_missing_executable_is_reported_by_path_and_a_missing_cwd_by_directory() -> TestResult {
    let missing = PathBuf::from("/nonexistent-claude-sdk-test/claude");
    let mut missing_cli = options("normal", ArgvStyle::Python)?;
    missing_cli.cli_path = Some(missing.clone());
    let error = ClaudeSdkClient::connect(missing_cli)
        .await
        .err()
        .ok_or_else(|| std::io::Error::other("expected a spawn error"))?;
    let Error::CliNotFoundAt(path) = &error else {
        return Err(std::io::Error::other(format!("unexpected error: {error:?}")).into());
    };
    assert_eq!(path, &missing);
    assert_eq!(
        error.to_string(),
        "Claude Code not found at: /nonexistent-claude-sdk-test/claude"
    );

    let mut missing_cwd = options("normal", ArgvStyle::Python)?;
    missing_cwd.cwd = Some(PathBuf::from("/nonexistent-claude-sdk-test/cwd"));
    let error = ClaudeSdkClient::connect(missing_cwd)
        .await
        .err()
        .ok_or_else(|| std::io::Error::other("expected a spawn error"))?;
    assert_eq!(
        error.to_string(),
        "Working directory does not exist: /nonexistent-claude-sdk-test/cwd"
    );
    Ok(())
}

#[tokio::test]
async fn abort_stops_the_process_and_ends_the_run_with_an_abort_error() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("blocked-stdin", ArgvStyle::TypeScript)?).await?;
    client.abort().await?;
    let error = timeout(Duration::from_secs(10), async {
        loop {
            match client.next_message().await {
                Ok(Some(_)) => {}
                Ok(None) => return None,
                Err(error) => return Some(error),
            }
        }
    })
    .await?;
    let Some(Error::Aborted(aborted)) = error else {
        return Err(std::io::Error::other(format!("expected an abort error, got {error:?}")).into());
    };
    assert_eq!(aborted.message(), "Claude Code process aborted by user");
    let _ = client.disconnect().await;
    Ok(())
}

#[tokio::test]
async fn t14_ending_input_after_the_run_failed_is_a_no_op_for_both_styles() -> TestResult {
    for style in [ArgvStyle::Python, ArgvStyle::TypeScript] {
        let mut options = options("malformed", style)?;
        options.message_decoding = MessageDecoding::Python;
        let mut client = ClaudeSdkClient::connect(options).await?;
        client.send_prompt(Prompt::from("go")).await?;
        let error = timeout(Duration::from_secs(10), async {
            loop {
                match client.next_message().await {
                    Ok(Some(_)) => {}
                    Ok(None) => return None,
                    Err(error) => return Some(error),
                }
            }
        })
        .await?;
        assert!(matches!(error, Some(Error::MessageParse(_))), "{style:?}: {error:?}");
        client.finish_input().await?;
        let _ = client.disconnect().await;
    }
    Ok(())
}
