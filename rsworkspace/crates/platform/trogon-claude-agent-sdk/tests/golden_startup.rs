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

use serde_json::Value;
use trogon_claude_agent_sdk::{Error, InitializeTimeoutError, Prompt, StartupOptions, startup_with_adapters};

#[path = "support/scripted_process.rs"]
mod scripted_process;
#[path = "support/startup_cli.rs"]
mod startup_cli;

use startup_cli::{
    COLD_ARGUMENTS, INIT, MARKS, Outcome, Script, WORK, arguments, hook_response, hook_started, mark, options, start,
    user,
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
    let query = warm.query(Prompt::from("hi")).await?;
    query.close().await?;
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

#[tokio::test]
async fn the_timeout_says_what_the_reported_hooks_did() -> Outcome {
    let cases: [(&[(&str, &str)], &[&str], String, usize); 6] = [
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
