#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Golden tests for `startup()`. The expected argv, environment flag, stdin lines and error texts were captured
//! from `sdk.mjs` 0.3.323 talking to a scripted CLI, with only the random request id normalized.

use std::{
    ffi::{OsStr, OsString},
    time::Duration,
};

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    Error, InitializeTimeoutError, Message, Prompt, StartupOptions, startup_with_adapters,
    upstream::messages::SDKStartupFailureReason,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;
#[path = "support/startup_cli.rs"]
mod startup_cli;

use startup_cli::{
    COLD_ARGUMENTS, HookGate, INIT, MARKS, Outcome, Script, WAIT, WORK, arguments, hook_response, hook_started, mark,
    options, start, user, uuid,
};

#[tokio::test]
async fn startup_writes_the_handshake_then_the_prompt() -> Outcome {
    let (adapters, run) = start(Script::default(), &[]);
    let warm = startup_with_adapters(StartupOptions::new(options(WORK)), adapters).await?;
    let command = run.command()?;
    assert_eq!(arguments(&command), COLD_ARGUMENTS);
    assert_eq!(
        command.environment.get(OsStr::new(MARKS)),
        Some(&Some(OsString::from("1")))
    );
    let mut query = warm.query(Prompt::from("hi")).await?;
    assert!(matches!(
        tokio::time::timeout(WAIT, query.next_message()).await??,
        Some(Message::Result(_))
    ));
    query.disconnect().await?;
    assert_eq!(run.lines().await?, vec![INIT.to_owned(), user("hi")]);
    Ok(())
}

#[tokio::test]
async fn startup_leaves_a_marks_flag_the_caller_already_has() -> Outcome {
    let (adapters, run) = start(Script::default(), &[(MARKS, "0")]);
    let warm = startup_with_adapters(StartupOptions::new(options(WORK)), adapters).await?;
    assert_ne!(
        run.command()?.environment.get(OsStr::new(MARKS)),
        Some(&Some(OsString::from("1")))
    );
    warm.close();
    run.lines().await?;
    Ok(())
}

#[tokio::test]
async fn closing_a_warm_query_sends_no_prompt() -> Outcome {
    let (adapters, run) = start(Script::default(), &[]);
    let warm = startup_with_adapters(StartupOptions::new(options(WORK)), adapters).await?;
    warm.close();
    assert_eq!(run.lines().await?, vec![INIT.to_owned()]);
    Ok(())
}

#[tokio::test]
async fn a_warm_query_reports_its_initialize_answer_and_disposes_without_a_prompt() -> Outcome {
    let (adapters, run) = start(Script::default(), &[]);
    let warm = startup_with_adapters(StartupOptions::new(options(WORK)), adapters).await?;
    let info = warm.connection_info().await?;
    assert_eq!(info.as_json(), Some(&json!({"hook_on_failure_supported": true})));
    warm.dispose().await?;
    assert_eq!(run.lines().await?, vec![INIT.to_owned()]);
    Ok(())
}

async fn timeout_error(script: Script) -> Outcome<InitializeTimeoutError> {
    let (adapters, run) = start(script, &[]);
    let params = StartupOptions {
        initialize_timeout: Some(Duration::from_millis(400)),
        ..StartupOptions::new(options(WORK))
    };
    let error = startup_with_adapters(params, adapters).await.err();
    run.server.abort();
    match error {
        Some(Error::InitializeTimeout(error)) => Ok(error),
        other => Err(format!("expected an initialize timeout, got {other:?}").into()),
    }
}

const WITHIN: &str = "Subprocess initialization did not complete within 400ms.";
const SLOW: &str =
    "Slow plugins, MCP servers, hooks that Claude Code does not report, disks or network can each cause this.";
const BACKGROUND: &str = "A hook that runs in the background counts as not finished until it ends.";
const UNREPORTED: &str =
    "Claude Code did not answer the initialize request and reported no SessionStart or Setup hook running.";

/// The default limit under a paused clock. The hook gate only observes: `onStartupHooksRunning` changes nothing the
/// timer counts.
async fn default_timeout_error(mut script: Script) -> Outcome<InitializeTimeoutError> {
    let gate = HookGate::default();
    script.hooks = Some(gate.clone());
    let (adapters, run) = start(script, &[]);
    let params = StartupOptions {
        on_startup_hooks_running: Some(gate.callback()),
        ..StartupOptions::new(options(WORK))
    };
    let error = startup_with_adapters(params, adapters).await.err();
    run.server.abort();
    match error {
        Some(Error::InitializeTimeout(error)) => Ok(error),
        other => Err(format!("expected an initialize timeout, got {other:?}").into()),
    }
}

const DEFAULT_WITHIN: &str = "Subprocess initialization did not complete within 60000ms.";

/// Expected texts captured live from `sdk.mjs` 0.3.323 with no timeout passed, a scripted CLI that never answers
/// initialize, and the hook frames sent at the offsets below.
#[tokio::test(start_paused = true)]
async fn the_default_timeout_waits_sixty_seconds_when_no_hook_is_reported() -> Outcome {
    let started = tokio::time::Instant::now();
    let error = default_timeout_error(Script {
        no_init: true,
        ..Script::default()
    })
    .await?;
    assert_eq!(started.elapsed(), Duration::from_secs(60));
    assert_eq!(error.message(), format!("{DEFAULT_WITHIN} {UNREPORTED} {SLOW}"));
    assert_eq!(error.telemetry_message(), DEFAULT_WITHIN.trim_end_matches('.'));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_default_timeout_does_not_count_one_hooks_time() -> Outcome {
    let started = tokio::time::Instant::now();
    let error = default_timeout_error(Script {
        no_init: true,
        timeline: vec![
            (Duration::ZERO, hook_started("h1", "SessionStart")),
            (Duration::from_secs(3), hook_response("h1", "SessionStart")),
        ],
        ..Script::default()
    })
    .await?;
    assert_eq!(started.elapsed(), Duration::from_secs(63));
    assert_eq!(
        error.message(),
        format!(
            "{DEFAULT_WITHIN} Not counted: 3 seconds in which Claude Code reported a SessionStart or Setup hook running. Claude Code reported 1 SessionStart hook, which finished, and then did not answer the initialize request. {SLOW}"
        )
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_default_timeout_counts_overlapping_hooks_once() -> Outcome {
    let started = tokio::time::Instant::now();
    let error = default_timeout_error(Script {
        no_init: true,
        timeline: vec![
            (Duration::ZERO, hook_started("h1", "SessionStart")),
            (Duration::ZERO, hook_started("h2", "Setup")),
            (Duration::from_secs(2), hook_response("h1", "SessionStart")),
            (Duration::from_secs(4), hook_response("h2", "Setup")),
        ],
        ..Script::default()
    })
    .await?;
    assert_eq!(started.elapsed(), Duration::from_secs(64));
    assert_eq!(
        error.message(),
        format!(
            "{DEFAULT_WITHIN} Not counted: 4 seconds in which Claude Code reported a SessionStart or Setup hook running. Claude Code reported 2 SessionStart and Setup hooks, which all finished, and then did not answer the initialize request. {SLOW}"
        )
    );
    Ok(())
}

/// Captured live from `sdk.mjs` 0.3.323: a first Setup hook that ran from 1s to 6s and a second that opened at 20s
/// and never ended. The uncounted time stops at 600 seconds, so the timeout fires at 660 seconds and names the
/// 600 seconds and the hook that is still open.
#[tokio::test(start_paused = true)]
async fn the_default_timeout_stops_not_counting_hook_time_after_ten_minutes() -> Outcome {
    let started = tokio::time::Instant::now();
    let error = default_timeout_error(Script {
        no_init: true,
        timeline: vec![
            (Duration::from_secs(1), hook_started("h1", "Setup")),
            (Duration::from_secs(6), hook_response("h1", "Setup")),
            (Duration::from_secs(20), hook_started("h2", "Setup")),
        ],
        ..Script::default()
    })
    .await?;
    assert_eq!(started.elapsed(), Duration::from_secs(660));
    assert_eq!(
        error.message(),
        format!(
            "{DEFAULT_WITHIN} Not counted: 600 seconds in which Claude Code reported a SessionStart or Setup hook running. Claude Code reported 2 Setup hooks, 1 of which had not finished, and did not answer the initialize request. {BACKGROUND} Check these hooks first, then authentication and network connectivity."
        )
    );
    assert_eq!(error.telemetry_message(), DEFAULT_WITHIN.trim_end_matches('.'));
    assert_eq!(error.startup_hooks_open(), 1);
    assert_eq!(error.startup_mark(), Some("none"));
    Ok(())
}

#[tokio::test]
async fn a_passed_timeout_counts_from_the_start_and_names_the_last_mark() -> Outcome {
    let script = Script {
        no_init: true,
        preface: vec![mark("loaded")],
        ..Script::default()
    };
    let error = timeout_error(script).await?;
    assert_eq!(error.message(), format!("{WITHIN} {UNREPORTED} {SLOW}"));
    assert_eq!(
        error.telemetry_message(),
        "Subprocess initialization did not complete within 400ms"
    );
    assert_eq!(error.startup_mark(), Some("loaded"));
    assert!(error.startup_mark_age_ms().is_some_and(|age| age <= 400));
    assert_eq!(error.startup_hooks_open(), 0);
    Ok(())
}

#[tokio::test]
async fn startup_marks_are_kept_up_to_sixty_four_characters() -> Outcome {
    let kept = "a".repeat(64);
    let error = timeout_error(Script {
        no_init: true,
        preface: vec![mark(&kept)],
        ..Script::default()
    })
    .await?;
    assert_eq!(error.startup_mark(), Some(kept.as_str()));
    let ignored = timeout_error(Script {
        no_init: true,
        preface: vec![mark(&"a".repeat(65))],
        ..Script::default()
    })
    .await?;
    assert_eq!(ignored.startup_mark(), Some("none"));
    assert_eq!(ignored.startup_mark_age_ms(), None);
    assert_eq!(ignored.message(), format!("{WITHIN} {UNREPORTED} {SLOW}"));
    Ok(())
}

/// Started hooks as `(id, event)`, the ids that answered, the expected message tail and how many hooks stay open.
type TimeoutCase = (
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
    String,
    usize,
);

#[tokio::test]
async fn the_timeout_says_what_the_reported_hooks_did() -> Outcome {
    let cases: [TimeoutCase; 6] = [
        (
            &[("h1", "SessionStart"), ("h2", "Setup")],
            &[],
            format!(
                "Claude Code reported 2 SessionStart and Setup hooks, none of which had finished, and did not answer the initialize request. {BACKGROUND} Check these hooks first, then authentication and network connectivity."
            ),
            2,
        ),
        (
            &[("h1", "SessionStart")],
            &[],
            format!(
                "Claude Code reported 1 SessionStart hook, which had not finished, and did not answer the initialize request. {BACKGROUND} Check this hook first, then authentication and network connectivity."
            ),
            1,
        ),
        (
            &[("h1", "Setup")],
            &["h1"],
            format!(
                "Claude Code reported 1 Setup hook, which finished, and then did not answer the initialize request. {SLOW}"
            ),
            0,
        ),
        (
            &[("h1", "Setup"), ("h2", "Setup")],
            &["h1", "h2"],
            format!(
                "Claude Code reported 2 Setup hooks, which all finished, and then did not answer the initialize request. {SLOW}"
            ),
            0,
        ),
        (
            &[("h1", "Setup"), ("h2", "SessionStart"), ("h3", "Setup")],
            &["h2"],
            format!(
                "Claude Code reported 3 Setup and SessionStart hooks, 2 of which had not finished, and did not answer the initialize request. {BACKGROUND} Check these hooks first, then authentication and network connectivity."
            ),
            2,
        ),
        (&[("h1", "PreToolUse")], &[], format!("{UNREPORTED} {SLOW}"), 0),
    ];
    for (hooks, responded, tail, open) in cases {
        let mut preface: Vec<Value> = hooks.iter().map(|(id, event)| hook_started(id, event)).collect();
        for id in responded {
            let event = hooks
                .iter()
                .find(|(hook, _)| hook == id)
                .map_or("SessionStart", |(_, event)| *event);
            preface.push(hook_response(id, event));
        }
        let error = timeout_error(Script {
            no_init: true,
            preface,
            ..Script::default()
        })
        .await?;
        assert_eq!(error.message(), format!("{WITHIN} {tail}"));
        assert_eq!(error.startup_hooks_open(), open);
        assert_eq!(error.startup_mark(), Some("none"));
    }
    Ok(())
}

fn refusal(reason: Option<Value>, subtype: &str, errors: Value) -> Value {
    let mut frame = json!({
        "type": "result",
        "subtype": subtype,
        "is_error": true,
        "duration_ms": 1,
        "duration_api_ms": 1,
        "num_turns": 0,
        "session_id": "s",
        "uuid": uuid(2)
    });
    if let Value::Object(object) = &mut frame {
        if subtype == "success" {
            object.insert("result".into(), errors);
        } else {
            object.insert("errors".into(), errors);
        }
        if let Some(reason) = reason {
            object.insert("startup_failure_reason".into(), reason);
        }
    }
    frame
}

/// A CLI that answers initialize with an error result and exits, so `startup()` rejects.
async fn refused(frame: Value, exit_code: i32) -> Outcome<trogon_claude_agent_sdk::ResultError> {
    let script = Script {
        error_on_init: Some(frame),
        exit_code,
        ..Script::default()
    };
    let (adapters, _run) = start(script, &[]);
    match tokio::time::timeout(
        WAIT,
        startup_with_adapters(StartupOptions::new(options(WORK)), adapters),
    )
    .await?
    {
        Err(Error::Result(result)) => Ok(result),
        Err(other) => Err(format!("expected a result error, got {other:?}").into()),
        Ok(_) => Err("startup succeeded".into()),
    }
}

#[tokio::test]
async fn an_error_result_exit_names_why_startup_failed() -> Outcome {
    let result = refused(
        refusal(
            Some(json!("cwd_unavailable")),
            "error_during_execution",
            json!(["boom"]),
        ),
        1,
    )
    .await?;
    assert_eq!(result.message(), "Claude Code returned an error result: boom");
    assert_eq!(
        result.startup_failure_reason(),
        Some(SDKStartupFailureReason::CwdUnavailable)
    );
    Ok(())
}

#[tokio::test]
async fn a_reason_the_sdk_does_not_list_is_kept_and_a_non_string_is_dropped() -> Outcome {
    let result = refused(
        refusal(
            Some(json!("some_new_reason")),
            "error_during_execution",
            json!(["boom"]),
        ),
        1,
    )
    .await?;
    assert_eq!(
        result.startup_failure_reason(),
        Some(SDKStartupFailureReason::Unknown("some_new_reason".to_owned()))
    );
    let result = refused(refusal(Some(json!(7)), "error_during_execution", json!(["boom"])), 1).await?;
    assert_eq!(result.message(), "Claude Code returned an error result: boom");
    assert_eq!(result.startup_failure_reason(), None);
    Ok(())
}

#[tokio::test]
async fn the_error_result_text_trims_and_joins_the_errors_or_uses_the_result_of_a_success_subtype() -> Outcome {
    let result = refused(refusal(None, "error_during_execution", json!([" a ", "", "b"])), 2).await?;
    assert_eq!(result.message(), "Claude Code returned an error result: a; b");
    assert_eq!(result.startup_failure_reason(), None);
    let result = refused(refusal(Some(json!("cwd_unavailable")), "success", json!("refused")), 1).await?;
    assert_eq!(result.message(), "Claude Code returned an error result: refused");
    assert_eq!(
        result.startup_failure_reason(),
        Some(SDKStartupFailureReason::CwdUnavailable)
    );
    Ok(())
}

/// Runs `WarmQuery::query` against a CLI that answers the prompt with an error result and then exits with
/// `exit_code` once its input ended, and returns how many messages the query yielded and how it ended.
async fn prompted_error_result(exit_code: i32) -> Outcome<(usize, Option<Error>)> {
    let script = Script {
        error_on_user: Some(refusal(
            Some(json!("cwd_unavailable")),
            "error_during_execution",
            json!(["boom"]),
        )),
        exit_code,
        ..Script::default()
    };
    let (adapters, _run) = start(script, &[]);
    let warm = startup_with_adapters(StartupOptions::new(options(WORK)), adapters).await?;
    let mut query = warm.query(Prompt::from("hi")).await?;
    let mut messages = 0;
    loop {
        match tokio::time::timeout(WAIT, query.next_message()).await? {
            Ok(Some(_)) => messages += 1,
            Ok(None) => return Ok((messages, None)),
            Err(error) => return Ok((messages, Some(error))),
        }
    }
}

/// Captured live from `sdk.mjs` 0.3.323: the error result reaches the caller as a message, and the stream then
/// throws the replacement error once the process exits with a nonzero code.
#[tokio::test]
async fn an_error_result_after_a_prompt_is_a_message_and_a_nonzero_exit_then_names_it() -> Outcome {
    let (messages, error) = prompted_error_result(1).await?;
    assert_eq!(messages, 1);
    match error {
        Some(Error::Result(result)) => {
            assert_eq!(result.message(), "Claude Code returned an error result: boom");
            assert_eq!(
                result.startup_failure_reason(),
                Some(SDKStartupFailureReason::CwdUnavailable)
            );
            Ok(())
        }
        other => Err(format!("expected a result error, got {other:?}").into()),
    }
}

/// Captured live from `sdk.mjs` 0.3.323: when the process exits with code 0 after the error result, the stream
/// ends without an error.
#[tokio::test]
async fn an_error_result_after_a_prompt_ends_the_stream_cleanly_when_the_process_exits_zero() -> Outcome {
    let (messages, error) = prompted_error_result(0).await?;
    assert_eq!(messages, 1);
    assert!(error.is_none(), "unexpected error {error:?}");
    Ok(())
}
