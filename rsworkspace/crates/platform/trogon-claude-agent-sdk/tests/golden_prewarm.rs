#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Golden tests for `prewarm()` and `SpareProcess::claim`. The expected argv, stdin lines, write order and error
//! texts were captured from `sdk.mjs` 0.3.323 talking to a scripted CLI, with only the random request id
//! normalized. A scripted CLI that holds its answers until it has read the lines the SDK writes right after the
//! request shows which writes happen before an answer and which wait for one.

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    os::unix::fs::PermissionsExt,
    time::Duration,
};

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    AgentDefinition, AgentName, ClaimError, ClaimInfo, ClaimOptions, ClaudeAgentOptions, ContinueConversation, Error,
    ExtraArgument, FeatureSupport, JsRuntime, PermissionMode, Prompt, SessionFork, SessionId, SpareProcess,
    StartupOptions, WorkspaceTrust, prewarm_with_adapters, upstream::messages::SDKStartupFailureReason,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;
#[path = "support/startup_cli.rs"]
mod startup_cli;

use startup_cli::*;

const SPARE: &str = "/spare";

async fn spare(script: Script) -> Outcome<(SpareProcess, Run)> {
    spare_under(script, JsRuntime::Node).await
}

async fn spare_under(script: Script, js_runtime: JsRuntime) -> Outcome<(SpareProcess, Run)> {
    let (adapters, run) = start(script, &[]);
    let options = ClaudeAgentOptions {
        js_runtime,
        ..options(SPARE)
    };
    let spare = prewarm_with_adapters(StartupOptions::new(options), adapters).await?;
    Ok((spare, run))
}

/// Claims a spare, then returns the initialization result as JSON text or the error text, and the error text of
/// `supported_models`, `None` when it succeeds.
async fn claimed_initialization(script: Script, js_runtime: JsRuntime) -> Outcome<(String, Option<String>)> {
    let (spare, run) = spare_under(script, js_runtime).await?;
    let query = spare.claim(Prompt::from("hello"), ClaimOptions::new(SPARE)).await?;
    spare.claimed().await?;
    let handle = query.handle().ok_or("the query has no handle")?;
    let result = match handle.initialization_result().await {
        Ok(info) => serde_json::to_string(&info.as_json())?,
        Err(error) => error.to_string(),
    };
    let models = handle.supported_models().await.err().map(|error| error.to_string());
    query.disconnect().await?;
    run.lines().await?;
    Ok((result, models))
}

struct Claimed {
    outcome: Result<ClaimInfo, ClaimError>,
    lines: Vec<String>,
}

/// Claims a spare with `hello`, waits for the claim to settle and returns what the CLI read.
async fn claimed(script: Script, claim: ClaimOptions) -> Outcome<Claimed> {
    let (spare, run) = spare(script).await?;
    let query = spare.claim(Prompt::from("hello"), claim).await?;
    let outcome = spare.claimed().await;
    let _ = query.disconnect().await;
    Ok(Claimed {
        outcome,
        lines: run.lines().await?,
    })
}

fn failure(outcome: Result<ClaimInfo, ClaimError>) -> Outcome<ClaimError> {
    outcome.err().ok_or_else(|| "the claim was expected to fail".into())
}

const WITHIN: &str = "Subprocess initialization did not complete within 400ms.";

#[tokio::test]
async fn a_spare_times_out_with_the_startup_texts_and_marks() -> Outcome {
    let (adapters, run) = start(
        Script {
            no_init: true,
            preface: vec![mark("loaded")],
            ..Script::default()
        },
        &[],
    );
    let params = StartupOptions {
        initialize_timeout: Some(Duration::from_millis(400)),
        ..StartupOptions::new(options(WORK))
    };
    let error = prewarm_with_adapters(params, adapters).await.err();
    run.server.abort();
    let Some(Error::InitializeTimeout(error)) = error else {
        return Err(format!("expected an initialize timeout, got {error:?}").into());
    };
    assert!(error.message().starts_with(WITHIN));
    assert_eq!(error.startup_mark(), Some("loaded"));
    assert_eq!(error.startup_hooks_open(), 0);
    let ignored = start(
        Script {
            no_init: true,
            preface: vec![mark(&"a".repeat(65))],
            ..Script::default()
        },
        &[],
    );
    let params = StartupOptions {
        initialize_timeout: Some(Duration::from_millis(400)),
        ..StartupOptions::new(options(WORK))
    };
    let error = prewarm_with_adapters(params, ignored.0).await.err();
    ignored.1.server.abort();
    let Some(Error::InitializeTimeout(error)) = error else {
        return Err(format!("expected an initialize timeout, got {error:?}").into());
    };
    assert_eq!(error.startup_mark(), Some("none"));
    Ok(())
}

#[tokio::test]
async fn prewarm_rejects_options_that_describe_a_session() -> Outcome {
    let message = "prewarm(): resume/continue/forkSession describe a session; a spare has none \u{2014} pass them to query() instead";
    let resume = ClaudeAgentOptions {
        resume: Some(SessionId::from("x")),
        ..options(SPARE)
    };
    let carry_on = ClaudeAgentOptions {
        continue_conversation: ContinueConversation::Yes,
        ..options(SPARE)
    };
    let fork = ClaudeAgentOptions {
        fork_session: SessionFork::Fork,
        ..options(SPARE)
    };
    for options in [resume, carry_on, fork] {
        let (adapters, run) = start(Script::default(), &[]);
        let error = prewarm_with_adapters(StartupOptions::new(options), adapters)
            .await
            .err();
        assert_eq!(error.map(|error| error.to_string()).as_deref(), Some(message));
        assert_eq!(run.spawned()?, 0);
        run.server.abort();
    }
    Ok(())
}

#[tokio::test]
async fn a_plain_claim_writes_the_claim_and_the_prompt_before_any_answer() -> Outcome {
    let (spare, run) = spare(Script {
        hold: 2,
        ..Script::default()
    })
    .await?;
    let command = run.command()?;
    let mut expected = COLD_ARGUMENTS.map(str::to_owned).to_vec();
    expected.push("--await-claim".to_owned());
    assert_eq!(arguments(&command), expected);
    assert_eq!(
        command.environment.get(OsStr::new(MARKS)),
        Some(&Some(OsString::from("1")))
    );
    let query = spare.claim(Prompt::from("hello"), ClaimOptions::new(SPARE)).await?;
    let info = spare.claimed().await?;
    assert_eq!(
        info,
        ClaimInfo {
            cwd: SPARE.to_owned(),
            session_id: "sess-1".to_owned(),
            parked_ms: Some(12.0),
            sdk_mcp_settled: true,
        }
    );
    query.disconnect().await?;
    assert_eq!(
        run.lines().await?,
        vec![INIT.to_owned(), claim_line(SPARE), user("hello")]
    );
    Ok(())
}

#[tokio::test]
async fn every_claim_option_keeps_the_upstream_key_order() -> Outcome {
    let mut agents = BTreeMap::new();
    agents.insert(AgentName::from("r"), AgentDefinition::new("d", Prompt::from("p")));
    let mut claim = ClaimOptions::new(SPARE);
    claim.permission_mode = Some(PermissionMode::Plan);
    claim.env = Some(vec![("B".into(), "2".into()), ("A".into(), "1".into())]);
    claim.additional_directories = Some(vec!["/x".into(), "/y".into()]);
    claim.append_system_prompt = Some("tail".into());
    claim.title = Some("T".into());
    claim.agents = Some(agents);
    claim.workspace_trust = Some(WorkspaceTrust {
        accepted: Some(true),
        directory: Some("/trusted".into()),
    });
    let result = claimed(
        Script {
            hold: 2,
            ..Script::default()
        },
        claim,
    )
    .await?;
    assert!(result.outcome.is_ok());
    assert_eq!(
        result.lines[1],
        control(
            r#"{"subtype":"claim_session","cwd":"/spare","permission_mode":"plan","env":{"B":"2","A":"1"},"additional_directories":["/x","/y"],"append_system_prompt":"tail","title":"T","agents":{"r":{"description":"d","prompt":"p"}},"workspace_trust":{"accepted":true,"directory":"/trusted"},"sdk_mcp_servers":[],"include_initialize":true}"#
        )
    );
    Ok(())
}

#[tokio::test]
async fn a_claim_that_sets_the_model_and_thinking_writes_them_before_the_prompt() -> Outcome {
    for (tokens, wire) in [(None, "null"), (Some(4000), "4000")] {
        let mut claim = ClaimOptions::new(SPARE);
        claim.model = Some("opus".into());
        claim.max_thinking_tokens = Some(tokens);
        let result = claimed(
            Script {
                hold: 4,
                ..Script::default()
            },
            claim,
        )
        .await?;
        assert!(result.outcome.is_ok());
        assert_eq!(
            result.lines,
            vec![
                INIT.to_owned(),
                claim_line(SPARE),
                control(r#"{"subtype":"set_model","model":"opus"}"#),
                control(&format!(
                    r#"{{"subtype":"set_max_thinking_tokens","max_thinking_tokens":{wire}}}"#
                )),
                user("hello"),
            ]
        );
    }
    Ok(())
}

#[tokio::test]
async fn settings_surround_the_claim_and_the_prompt_waits_for_every_answer() -> Outcome {
    let mut claim = ClaimOptions::new(SPARE);
    claim.model = Some("opus".into());
    claim.settings = Some(object(json!({
        "model":"x","permissions":{"allow":["Read"]},"env":{"K":"v"},"agent":"a","extra":1
    })));
    let result = claimed(
        Script {
            hold: 4,
            ..Script::default()
        },
        claim,
    )
    .await?;
    assert!(result.outcome.is_ok());
    assert_eq!(
        result.lines,
        vec![
            INIT.to_owned(),
            control(r#"{"subtype":"apply_flag_settings","settings":{"env":{"K":"v"},"extra":1}}"#),
            claim_line(SPARE),
            control(r#"{"subtype":"set_model","model":"opus"}"#),
            control(
                r#"{"subtype":"apply_flag_settings","settings":{"model":"x","agent":"a","permissions":{"allow":["Read"]}}}"#
            ),
            user("hello"),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn failed_settings_close_the_process_without_sending_the_prompt() -> Outcome {
    let mut claim = ClaimOptions::new(SPARE);
    claim.settings = Some(object(json!({"env":{"K":"v"}})));
    let script = Script {
        hold: 2,
        fail: vec!["apply_flag_settings"],
        ..Script::default()
    };
    let result = claimed(script, claim).await?;
    let error = failure(result.outcome)?;
    assert_eq!(
        error.to_string(),
        "settings_not_applied: apply_flag_settings failed \u{2014} the prompt was not sent and the process was closed; start the session with query()"
    );
    assert_eq!(error.telemetry_message(), Some("settings_not_applied"));
    assert!(error.claim().is_some_and(|claim| claim.cwd == SPARE));
    assert_eq!(
        result.lines,
        vec![
            INIT.to_owned(),
            control(r#"{"subtype":"apply_flag_settings","settings":{"env":{"K":"v"}}}"#),
            claim_line(SPARE),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn failed_options_report_what_was_not_applied_after_sending_the_prompt() -> Outcome {
    let cases: [(&[&'static str], &str); 2] = [
        (&["set_model"], "model: set_model failed"),
        (
            &["set_model", "set_max_thinking_tokens"],
            "model: set_model failed; maxThinkingTokens: set_max_thinking_tokens failed",
        ),
    ];
    for (fail, reasons) in cases {
        let mut claim = ClaimOptions::new(SPARE);
        claim.model = Some("opus".into());
        claim.max_thinking_tokens = Some(Some(5));
        let script = Script {
            hold: 4,
            fail: fail.to_vec(),
            ..Script::default()
        };
        let result = claimed(script, claim).await?;
        let error = failure(result.outcome)?;
        assert_eq!(
            error.to_string(),
            format!(
                "option_not_applied: {reasons} \u{2014} the prompt was sent and the session runs without these options"
            )
        );
        assert_eq!(error.telemetry_message(), Some("option_not_applied"));
        assert!(error.claim().is_some());
        assert_eq!(result.lines.last(), Some(&user("hello")));
    }
    Ok(())
}

#[tokio::test]
async fn a_refused_claim_carries_the_cli_text_and_still_gets_a_result() -> Outcome {
    let script = Script {
        hold: 2,
        claim_error: Some("not_a_spare: nope"),
        ..Script::default()
    };
    let (spare, run) = spare(script).await?;
    let mut query = spare.claim(Prompt::from("hello"), ClaimOptions::new(SPARE)).await?;
    let error = failure(spare.claimed().await)?;
    assert_eq!(error.to_string(), "not_a_spare: nope");
    assert_eq!(
        error.telemetry_message(),
        Some("Claude Code control request failed (claim_session)")
    );
    assert!(matches!(query.next_message().await, Ok(Some(_))));
    let _ = query.disconnect().await;
    assert_eq!(
        run.lines().await?,
        vec![INIT.to_owned(), claim_line(SPARE), user("hello")]
    );
    Ok(())
}

#[tokio::test]
async fn hook_settings_are_refused_without_sending_anything_and_the_spare_stays_claimable() -> Outcome {
    let (spare, run) = spare(Script {
        hold: 2,
        ..Script::default()
    })
    .await?;
    let mut claim = ClaimOptions::new(SPARE);
    claim.settings = Some(object(json!({"hooks":{},"disableAllHooks":true})));
    let error = spare
        .claim(Prompt::from("hello"), claim)
        .await
        .err()
        .ok_or("the claim was expected to fail")?;
    assert_eq!(
        error.to_string(),
        "SpareProcess.claim(): hook settings (hooks, disableAllHooks) cannot be applied at claim; pass them to prewarm() in Options.settings, where they are in force before any hook, as on a cold start. Nothing was sent: this spare can still be claimed without them."
    );
    assert_eq!(
        error.telemetry_message(),
        Some("SpareProcess.claim(): hook settings cannot be applied at claim")
    );
    let query = spare.claim(Prompt::from("hello"), ClaimOptions::new(SPARE)).await?;
    spare.claimed().await?;
    query.disconnect().await?;
    assert_eq!(
        run.lines().await?,
        vec![INIT.to_owned(), claim_line(SPARE), user("hello")]
    );
    Ok(())
}

#[tokio::test]
async fn a_spare_can_be_claimed_only_once_and_not_after_it_closed() -> Outcome {
    let (spare, run) = spare(Script {
        hold: 2,
        ..Script::default()
    })
    .await?;
    let query = spare.claim(Prompt::from("hello"), ClaimOptions::new(SPARE)).await?;
    let second = spare
        .claim(Prompt::from("again"), ClaimOptions::new(SPARE))
        .await
        .err()
        .ok_or("a second claim was expected to fail")?;
    assert_eq!(second.to_string(), "SpareProcess.claim() can only be called once");
    query.disconnect().await?;
    run.lines().await?;

    let (closed, run) = self::spare(Script::default()).await?;
    closed.close();
    let settled = failure(closed.claimed().await)?;
    assert_eq!(
        settled.to_string(),
        "spare_closed: the spare was closed before it was claimed"
    );
    let late = closed
        .claim(Prompt::from("a"), ClaimOptions::new(SPARE))
        .await
        .err()
        .ok_or("a claim after close was expected to fail")?;
    assert_eq!(
        late.to_string(),
        "SpareProcess.claim(): the spare was closed or has exited; start the session with query()"
    );
    assert_eq!(run.lines().await?, vec![INIT.to_owned()]);
    Ok(())
}

#[tokio::test]
async fn a_relative_claim_directory_resolves_against_the_process_and_a_home_one_is_sent_as_is() -> Outcome {
    let relative = claimed(
        Script {
            hold: 2,
            ..Script::default()
        },
        ClaimOptions::new("rel/dir"),
    )
    .await?;
    assert_eq!(relative.lines[1], claim_line("/work/rel/dir"));
    let home = claimed(
        Script {
            hold: 2,
            ..Script::default()
        },
        ClaimOptions::new("~/proj"),
    )
    .await?;
    assert_eq!(home.lines[1], claim_line("~/proj"));
    Ok(())
}

#[tokio::test]
async fn the_claim_answer_replaces_the_initialization_result_and_keeps_the_applied_flags() -> Outcome {
    let script = Script {
        hold: 2,
        init: Some(
            json!({"hooks_applied":true,"plugins_applied":true,"managed_settings_applied":true,
            "hook_on_failure_supported":true,"commands":[{"name":"a"}],"agents":[]}),
        ),
        claim_init: Some(json!({"commands":[{"name":"b"}],"agents":[{"name":"g"}]})),
        ..Script::default()
    };
    let (spare, run) = spare(script).await?;
    let query = spare.claim(Prompt::from("hello"), ClaimOptions::new(SPARE)).await?;
    spare.claimed().await?;
    let handle = query.handle().ok_or("the query has no handle")?;
    let info = handle.initialization_result().await?;
    assert_eq!(info.commands, vec![object(json!({"name":"b"}))]);
    assert_eq!(info.agents, vec![object(json!({"name":"g"}))]);
    assert_eq!(info.hooks_applied, FeatureSupport::Supported);
    assert_eq!(info.plugins_applied, FeatureSupport::Supported);
    assert_eq!(info.managed_settings_applied, FeatureSupport::Supported);
    query.disconnect().await?;
    run.lines().await?;
    Ok(())
}

/// `claimSession` reads the applied flags off the earlier initialization result before it reads the claim's
/// `initialize`. An initialize answered without a response or with `null`, or a claim `initialize` of `null`,
/// rejects the initialization result with the engine's `TypeError` while `claimed` still resolves.
#[tokio::test]
async fn a_claim_over_a_missing_initialize_response_rejects_the_initialization_result() -> Outcome {
    let cases: [(JsRuntime, Option<Value>, Value, &str); 6] = [
        (
            JsRuntime::Node,
            None,
            json!({"commands":[{"name":"b"}],"agents":[]}),
            "Cannot read properties of undefined (reading 'hooks_applied')",
        ),
        (
            JsRuntime::Node,
            Some(Value::Null),
            json!({"commands":[{"name":"b"}],"agents":[]}),
            "Cannot read properties of null (reading 'hooks_applied')",
        ),
        (
            JsRuntime::Node,
            Some(json!({"commands":[{"name":"a"}],"hooks_applied":true,"plugins_applied":false})),
            Value::Null,
            "Cannot read properties of null (reading 'hooks_applied')",
        ),
        (
            JsRuntime::Bun,
            None,
            json!({"commands":[{"name":"b"}]}),
            "undefined is not an object (evaluating 'd.hooks_applied')",
        ),
        (
            JsRuntime::Bun,
            Some(Value::Null),
            json!({"commands":[]}),
            "null is not an object (evaluating 'd.hooks_applied')",
        ),
        (
            JsRuntime::Bun,
            Some(json!({"hooks_applied":true})),
            Value::Null,
            "null is not an object (evaluating 'o.hooks_applied')",
        ),
    ];
    for (js_runtime, init, claim_init, expected) in cases {
        let script = Script {
            init_omitted: init.is_none(),
            init,
            claim_init: Some(claim_init),
            ..Script::default()
        };
        let (result, models) = claimed_initialization(script, js_runtime).await?;
        assert_eq!(result, expected);
        assert_eq!(models.as_deref(), Some(expected));
    }
    Ok(())
}

/// The claim's `initialize` is spread the way JavaScript spreads any value, and each applied flag it omits or sends
/// as `null` comes from the earlier result, a `null` there included.
#[tokio::test]
async fn a_claim_initialize_of_any_type_is_spread_and_keeps_the_earlier_flags() -> Outcome {
    let init = json!({"commands":[{"name":"a"}],"hooks_applied":true,"plugins_applied":false});
    let cases: [(Value, Option<Value>, &str); 7] = [
        (
            init.clone(),
            Some(json!("ab")),
            r#"{"0":"a","1":"b","hooks_applied":true,"plugins_applied":false}"#,
        ),
        (
            init.clone(),
            Some(json!(["x", "y"])),
            r#"{"0":"x","1":"y","hooks_applied":true,"plugins_applied":false}"#,
        ),
        (
            init.clone(),
            Some(json!(7)),
            r#"{"hooks_applied":true,"plugins_applied":false}"#,
        ),
        (
            init.clone(),
            Some(json!(true)),
            r#"{"hooks_applied":true,"plugins_applied":false}"#,
        ),
        (
            init.clone(),
            None,
            r#"{"commands":[{"name":"a"}],"hooks_applied":true,"plugins_applied":false}"#,
        ),
        (
            init,
            Some(json!({"hooks_applied":null,"plugins_applied":true,"managed_settings_applied":null})),
            r#"{"hooks_applied":true,"plugins_applied":true,"managed_settings_applied":null}"#,
        ),
        (
            json!({"hooks_applied":null}),
            Some(json!({"commands":[]})),
            r#"{"commands":[],"hooks_applied":null}"#,
        ),
    ];
    for (init, claim_init, expected) in cases {
        let script = Script {
            init: Some(init),
            claim_init_omitted: claim_init.is_none(),
            claim_init,
            ..Script::default()
        };
        let (result, _) = claimed_initialization(script, JsRuntime::Node).await?;
        assert_eq!(result, expected);
    }
    Ok(())
}

fn parked_name(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .and_then(|name| name.strip_prefix("spare-"))
        .is_some_and(|suffix| suffix.len() == 6 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

#[tokio::test]
async fn a_spare_without_a_directory_parks_under_the_config_home_until_it_is_claimed() -> Outcome {
    let root = tempfile::tempdir()?;
    let config = root.path().join("cfg");
    let config_text = config.to_string_lossy().into_owned();
    let (adapters, run) = start(
        Script {
            hold: 2,
            ..Script::default()
        },
        &[("CLAUDE_CONFIG_DIR", config_text.as_str())],
    );
    let unparked = ClaudeAgentOptions {
        cwd: None,
        ..options(SPARE)
    };
    let spare = prewarm_with_adapters(StartupOptions::new(unparked), adapters).await?;
    let parked = run
        .command()?
        .directory
        .ok_or("the spare was not started in a directory")?;
    let spares = config.join("spares");
    assert_eq!(parked.parent(), Some(spares.as_path()));
    assert!(parked_name(&parked));
    assert!(parked.is_dir());
    assert_eq!(std::fs::metadata(&spares)?.permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(&parked)?.permissions().mode() & 0o777, 0o700);
    let query = spare.claim(Prompt::from("hello"), ClaimOptions::new(SPARE)).await?;
    spare.claimed().await?;
    assert_eq!(std::fs::read_dir(&spares)?.count(), 0);
    query.disconnect().await?;
    run.lines().await?;
    Ok(())
}

#[tokio::test]
async fn a_relative_config_home_falls_back_to_the_process_home() -> Outcome {
    let root = tempfile::tempdir()?;
    let home = root.path().to_string_lossy().into_owned();
    let (adapters, run) = start(Script::default(), &[("HOME", home.as_str())]);
    let mut unparked = ClaudeAgentOptions {
        cwd: None,
        ..options(SPARE)
    };
    unparked.env.insert("CLAUDE_CONFIG_DIR".into(), "rel2".into());
    unparked.env.insert("HOME".into(), "rel".into());
    let spare = prewarm_with_adapters(StartupOptions::new(unparked), adapters).await?;
    let parked = run
        .command()?
        .directory
        .ok_or("the spare was not started in a directory")?;
    assert_eq!(
        parked.parent(),
        Some(root.path().join(".claude").join("spares").as_path())
    );
    assert!(parked_name(&parked));
    spare.close();
    run.lines().await?;
    Ok(())
}

/// Claims a spare with a CLI that answers the prompt with an error result and exits with `exit_code` once its
/// input ended, and returns how many messages the query yielded and how it ended.
async fn claimed_error_result(exit_code: i32) -> Outcome<(usize, Option<Error>)> {
    let frame = json!({"type":"result","subtype":"error_during_execution","is_error":true,"duration_ms":1,
        "duration_api_ms":1,"num_turns":0,"session_id":"s","errors":["boom"],"startup_failure_reason":"cwd_unavailable",
        "uuid":uuid(2)});
    let (spare, _run) = spare(Script {
        error_on_user: Some(frame),
        exit_code,
        ..Script::default()
    })
    .await?;
    let mut query = spare.claim(Prompt::from("hello"), ClaimOptions::new(SPARE)).await?;
    let mut messages = 0;
    loop {
        match tokio::time::timeout(WAIT, query.next_message()).await? {
            Ok(Some(_)) => messages += 1,
            Ok(None) => return Ok((messages, None)),
            Err(error) => return Ok((messages, Some(error))),
        }
    }
}

/// Captured live from `sdk.mjs` 0.3.323: a claimed spare treats an error result after the prompt like a
/// `WarmQuery`. The result is a message, then a nonzero exit throws the replacement error and a zero exit ends
/// the stream cleanly.
#[tokio::test]
async fn a_claimed_spare_yields_the_error_result_then_names_it_on_a_nonzero_exit() -> Outcome {
    let (messages, error) = claimed_error_result(1).await?;
    assert_eq!(messages, 1);
    match error {
        Some(Error::Result(result)) => {
            assert_eq!(result.message(), "Claude Code returned an error result: boom");
            assert_eq!(
                result.startup_failure_reason(),
                Some(SDKStartupFailureReason::CwdUnavailable)
            );
        }
        other => return Err(format!("expected a result error, got {other:?}").into()),
    }
    let (messages, error) = claimed_error_result(0).await?;
    assert_eq!(messages, 1);
    assert!(error.is_none(), "unexpected error {error:?}");
    Ok(())
}

#[tokio::test]
async fn closing_a_parked_spare_twice_then_disposing_it_removes_its_directory() -> Outcome {
    let root = tempfile::tempdir()?;
    let config = root.path().join("cfg");
    let config_text = config.to_string_lossy().into_owned();
    let (adapters, run) = start(Script::default(), &[("CLAUDE_CONFIG_DIR", config_text.as_str())]);
    let unparked = ClaudeAgentOptions {
        cwd: None,
        ..options(SPARE)
    };
    let spare = prewarm_with_adapters(StartupOptions::new(unparked), adapters).await?;
    let parked = run
        .command()?
        .directory
        .ok_or("the spare was not started in a directory")?;
    assert!(parked.is_dir());
    spare.close();
    spare.close();
    spare.dispose().await;
    tokio::time::timeout(WAIT, async {
        while parked.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    let settled = failure(spare.claimed().await)?;
    assert!(matches!(settled, ClaimError::SpareClosed), "{settled:?}");
    assert_eq!(run.lines().await?, vec![INIT.to_owned()]);
    Ok(())
}

#[tokio::test]
async fn a_spare_drops_workspace_trust_and_sends_await_claim_without_a_value() -> Outcome {
    let (adapters, run) = start(Script::default(), &[]);
    let spare_options = ClaudeAgentOptions {
        workspace_trust: Some(WorkspaceTrust {
            accepted: Some(true),
            directory: Some("/trusted".into()),
        }),
        extra_args: vec![ExtraArgument {
            flag: "await-claim".into(),
            value: Some("x".into()),
        }],
        ..options(SPARE)
    };
    let spare = prewarm_with_adapters(StartupOptions::new(spare_options), adapters).await?;
    let mut expected = COLD_ARGUMENTS.map(str::to_owned).to_vec();
    expected.push("--await-claim".to_owned());
    assert_eq!(arguments(&run.command()?), expected);
    spare.close();
    assert_eq!(run.lines().await?, vec![INIT.to_owned()]);
    Ok(())
}

#[tokio::test]
async fn the_native_entry_points_report_a_missing_executable_by_path() -> Outcome {
    let root = tempfile::tempdir()?;
    let missing = std::path::PathBuf::from("/nonexistent-claude-sdk-test/claude");
    let native = ClaudeAgentOptions {
        cli_path: Some(missing.clone()),
        cwd: Some(root.path().to_path_buf()),
        ..ClaudeAgentOptions::default()
    };
    let warm = trogon_claude_agent_sdk::startup(StartupOptions::new(native.clone()))
        .await
        .err();
    assert!(
        matches!(&warm, Some(Error::CliNotFoundAt(path)) if *path == missing),
        "{warm:?}"
    );
    let spare = trogon_claude_agent_sdk::prewarm(StartupOptions::new(native))
        .await
        .err();
    assert!(
        matches!(&spare, Some(Error::CliNotFoundAt(path)) if *path == missing),
        "{spare:?}"
    );
    Ok(())
}
