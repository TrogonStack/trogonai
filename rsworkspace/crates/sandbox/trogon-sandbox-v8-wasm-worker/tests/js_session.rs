mod support;

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use trogon_sandbox_v8_wasm_worker::{GuestOutcome, JsRuntimeError, JsSession};

use support::{TestResult, cpu_permit};

const T: Duration = Duration::from_secs(5);

async fn completed_text(
    session: &JsSession,
    source: &str,
    cancel: &CancellationToken,
) -> Result<String, Box<dyn std::error::Error>> {
    match session.eval(source.to_string(), T, cancel).await {
        GuestOutcome::Completed { output } => Ok(output.text()),
        other => Err(format!("expected Completed, got {other:?}").into()),
    }
}

#[tokio::test]
async fn shutdown_joins_the_thread_even_while_an_eval_borrows_the_session() -> TestResult {
    let _permit = cpu_permit().await?;
    let session = JsSession::start(64)?;
    let cancel = CancellationToken::new();
    let eval = session.eval("while (true) {}".into(), Duration::from_secs(60), &cancel);
    tokio::pin!(eval);
    assert!(futures::poll!(eval.as_mut()).is_pending());
    session.shutdown().await?;
    assert!(session.is_closed(), "the native thread must have actually joined");
    assert!(matches!(
        eval.await,
        GuestOutcome::Cancelled { .. } | GuestOutcome::Failed { .. }
    ));
    assert!(matches!(
        session.eval("1".into(), Duration::from_secs(1), &cancel).await,
        GuestOutcome::Cancelled { .. }
    ));
    session.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn session_state_persists_across_evals() -> TestResult {
    let _permit = cpu_permit().await?;
    let session = JsSession::start(64)?;
    let cancel = CancellationToken::new();
    assert_eq!(completed_text(&session, "globalThis.n = 41;", &cancel).await?, "");
    assert_eq!(
        completed_text(&session, "globalThis.n += 1; console.log(globalThis.n);", &cancel).await?,
        "42\n"
    );
    assert!(!session.is_retired());
    assert!(!session.is_closed());
    Ok(())
}

#[tokio::test]
async fn session_survives_a_thrown_error_and_keeps_state() -> TestResult {
    let _permit = cpu_permit().await?;
    let session = JsSession::start(64)?;
    let cancel = CancellationToken::new();
    completed_text(&session, "globalThis.kept = 'yes';", &cancel).await?;
    let failed = session.eval("throw new Error('boom');".to_string(), T, &cancel).await;
    assert!(matches!(failed, GuestOutcome::Failed { .. }));
    assert_eq!(
        completed_text(&session, "console.log(globalThis.kept);", &cancel).await?,
        "yes\n"
    );
    Ok(())
}

#[tokio::test]
async fn session_timeout_terminates_the_script_and_keeps_the_runtime() -> TestResult {
    let _permit = cpu_permit().await?;
    let session = JsSession::start(64)?;
    let cancel = CancellationToken::new();
    completed_text(&session, "globalThis.v = 7;", &cancel).await?;
    let timed_out = session
        .eval("while(true) {}".to_string(), Duration::from_millis(300), &cancel)
        .await;
    assert!(matches!(timed_out, GuestOutcome::TimedOut { .. }));
    assert_eq!(
        completed_text(&session, "console.log(globalThis.v);", &cancel).await?,
        "7\n"
    );
    Ok(())
}

#[tokio::test]
async fn session_cancel_terminates_the_script_and_keeps_the_runtime() -> TestResult {
    let _permit = cpu_permit().await?;
    let session = JsSession::start(64)?;
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        trigger.cancel();
    });
    let outcome = session
        .eval("while(true) {}".to_string(), Duration::from_secs(60), &cancel)
        .await;
    assert!(matches!(outcome, GuestOutcome::Cancelled { .. }));
    assert_eq!(
        completed_text(&session, "console.log(1);", &CancellationToken::new()).await?,
        "1\n"
    );
    Ok(())
}

#[tokio::test]
async fn session_is_retired_instead_of_crashing_when_the_heap_limit_is_hit() -> TestResult {
    let _permit = cpu_permit().await?;
    let session = JsSession::start(16)?;
    let cancel = CancellationToken::new();
    let outcome = session
        .eval(
            "globalThis.a = []; while(true) { globalThis.a.push(new Array(100000).fill(1)); }".to_string(),
            Duration::from_secs(30),
            &cancel,
        )
        .await;
    let GuestOutcome::Failed { error, .. } = outcome else {
        return Err(format!("expected Failed, got {outcome:?}").into());
    };
    assert!(error.contains("memory limit"), "error: {error}");
    assert!(session.is_retired());
    let after = session.eval("1".to_string(), Duration::from_secs(1), &cancel).await;
    assert!(matches!(after, GuestOutcome::Failed { error, .. } if error == "runtime is retired"));
    Ok(())
}

#[tokio::test]
async fn session_has_no_ambient_authority() -> TestResult {
    let _permit = cpu_permit().await?;
    let session = JsSession::start(64)?;
    assert_eq!(
        completed_text(&session, "console.log(typeof Deno);", &CancellationToken::new()).await?,
        "undefined\n"
    );
    Ok(())
}

#[test]
fn starting_a_session_without_a_tokio_runtime_is_refused() {
    assert!(matches!(JsSession::start(64), Err(JsRuntimeError::NoTokioRuntime)));
}
