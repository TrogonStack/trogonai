#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Tests exercise exact upstream JSON values.")
)]

use serde_json::json;

use super::{claim_initialization, connection_failure};
use crate::{ControlOperation, Error, JsRuntime};

#[test]
fn startup_error_is_preserved_when_cleanup_succeeds_or_only_loses_the_driver() {
    for cleanup in [Ok(()), Err(Error::DriverStopped)] {
        assert!(matches!(
            connection_failure(Error::TooManyControls, cleanup),
            Error::TooManyControls
        ));
    }
    assert!(matches!(
        connection_failure(Error::DriverStopped, Ok(())),
        Error::DriverStopped
    ));
    assert!(matches!(
        connection_failure(Error::DriverStopped, Err(Error::UnexpectedEof)),
        Error::UnexpectedEof
    ));
}

#[test]
fn identical_startup_policy_failures_are_not_reported_twice() {
    assert!(matches!(
        connection_failure(Error::BlockingHooksUnsupported, Err(Error::BlockingHooksUnsupported)),
        Error::BlockingHooksUnsupported
    ));
    assert!(matches!(
        connection_failure(Error::HooksNotApplied, Err(Error::HooksNotApplied)),
        Error::HooksNotApplied
    ));
    assert!(matches!(
        connection_failure(Error::ManagedSettingsNotApplied, Err(Error::ManagedSettingsNotApplied)),
        Error::ManagedSettingsNotApplied
    ));
    assert!(matches!(
        connection_failure(
            Error::ControlTimeout {
                operation: ControlOperation::Initialize
            },
            Err(Error::ControlTimeout {
                operation: ControlOperation::Initialize
            })
        ),
        Error::ControlTimeout {
            operation: ControlOperation::Initialize
        }
    ));
}

#[test]
fn distinct_startup_and_cleanup_failures_retain_both_typed_causes() {
    let error = connection_failure(
        Error::ControlTimeout {
            operation: ControlOperation::Initialize,
        },
        Err(Error::ControlTimeout {
            operation: ControlOperation::Interrupt,
        }),
    );
    assert!(matches!(error, Error::Cleanup { primary, cleanup }
        if matches!(*primary, Error::ControlTimeout { operation: ControlOperation::Initialize })
        && matches!(*cleanup, Error::ControlTimeout { operation: ControlOperation::Interrupt })));
    assert!(matches!(
        connection_failure(Error::BlockingHooksUnsupported, Err(Error::HooksNotApplied)),
        Error::Cleanup { .. }
    ));
}

#[test]
fn a_claim_over_a_failed_or_unsettled_result_keeps_no_flags() -> Result<(), Box<dyn std::error::Error>> {
    let failed = Err(std::sync::Arc::new(Error::DriverStopped));
    let update = json!({"commands":[],"hooks_applied":null});
    for previous in [Some(&failed), None] {
        let merged = claim_initialization(previous, &update, JsRuntime::Node).map_err(|error| error.to_string())?;
        assert_eq!(merged, Some(update.clone()));
    }
    Ok(())
}

#[test]
fn a_claim_initialize_string_spreads_its_utf16_code_units() -> Result<(), Box<dyn std::error::Error>> {
    let previous = Ok(Some(json!({"hooks_applied":false})));
    let merged = claim_initialization(Some(&previous), &json!("a\u{1F600}"), JsRuntime::Bun)
        .map_err(|error| error.to_string())?;
    assert_eq!(
        merged,
        Some(json!({"0":"a","1":"\u{FFFD}","2":"\u{FFFD}","hooks_applied":false}))
    );
    Ok(())
}

/// A handle no driver serves: `initialize_watched` reads only the receivers it is given.
fn unserved_handle() -> super::ClientHandle {
    let (commands, _receiver) = crate::channel::channel(None);
    super::ClientHandle {
        commands,
        timeout: None,
        abort: crate::CancellationToken::default(),
        close: super::CloseSignal::new(tokio::sync::oneshot::channel().0),
        closed: super::Closed::default(),
        initialization: std::sync::Arc::new(tokio::sync::watch::channel(None).0),
        latest_commands: super::driver::LatestCommands::default(),
        prompt_hold: super::driver::PromptHold::default(),
        host: crate::remote_control::HostState::default(),
        style: crate::ArgvStyle::TypeScript,
        js_runtime: JsRuntime::default(),
        claim_pending: std::sync::Arc::new(tokio::sync::watch::channel(false).0),
    }
}

/// A startup watch whose driver side already stopped reporting frames.
fn closed_startup_watch() -> crate::startup_timeout::StartupWatch {
    let (_tap, events) = tokio::sync::mpsc::unbounded_channel();
    crate::startup_timeout::StartupWatch {
        settings: crate::startup_timeout::StartupSettings {
            initialize_timeout: None,
            on_hooks_running: None,
        },
        events,
    }
}

#[tokio::test]
async fn startup_still_takes_the_initialize_answer_after_the_frame_channel_closes()
-> Result<(), Box<dyn std::error::Error>> {
    let handle = unserved_handle();
    let (answer, initialize) = tokio::sync::oneshot::channel();
    let mut waiting = std::pin::pin!(handle.initialize_watched(initialize, closed_startup_watch()));
    // The closed frame channel is the only ready branch, so the first poll takes it and keeps waiting for initialize,
    // as TypeScript's `m.race(initializationResult())` does once no more hook frames can arrive.
    let first = std::future::poll_fn(|cx| std::task::Poll::Ready(waiting.as_mut().poll(cx))).await;
    assert!(first.is_pending());
    let response = json!({"commands": [], "models": []});
    answer
        .send(Ok(Some(response.clone())))
        .map_err(|_| "the initialize wait dropped its receiver")?;
    assert_eq!(waiting.await?, Some(response));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn startup_still_times_out_after_the_frame_channel_closes() {
    let handle = unserved_handle();
    let (_answer, initialize) = tokio::sync::oneshot::channel();
    let result = handle.initialize_watched(initialize, closed_startup_watch()).await;
    assert!(matches!(result, Err(Error::InitializeTimeout(_))));
}
