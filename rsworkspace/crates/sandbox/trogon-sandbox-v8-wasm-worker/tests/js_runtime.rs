mod support;

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::OutputStream;
use trogon_sandbox_v8_wasm_worker::{GuestOutcome, run_js};

use support::{TestResult, cpu_permit};

async fn run(source: &str, memory_mib: u32, timeout: Duration) -> Result<GuestOutcome, Box<dyn std::error::Error>> {
    Ok(run_js(source.to_string(), memory_mib, timeout, CancellationToken::new()).await?)
}

#[tokio::test]
async fn console_log_is_captured_without_touching_real_stdout() -> TestResult {
    let _permit = cpu_permit().await?;
    let outcome = run("console.log('hello from v8');", 64, Duration::from_secs(5)).await?;
    let GuestOutcome::Completed { output } = outcome else {
        return Err(format!("expected Completed, got {outcome:?}").into());
    };
    assert!(output.text().contains("hello from v8"));
    Ok(())
}

#[tokio::test]
async fn console_formats_every_level_and_non_string_values() -> TestResult {
    let _permit = cpu_permit().await?;
    let script = "console.debug({a: 1}); console.info('i'); console.warn('w'); console.error('e'); \
                  const c = {}; c.c = c; console.log(c);";
    let outcome = run(script, 64, Duration::from_secs(5)).await?;
    let GuestOutcome::Completed { output } = outcome else {
        return Err(format!("expected Completed, got {outcome:?}").into());
    };
    assert_eq!(
        output.lines,
        vec![
            (OutputStream::Stdout, "{\"a\":1}\n".to_string()),
            (OutputStream::Stdout, "i\n".to_string()),
            (OutputStream::Stderr, "w\n".to_string()),
            (OutputStream::Stderr, "e\n".to_string()),
            (OutputStream::Stdout, "[object Object]\n".to_string()),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn no_ambient_authority_leaks_to_guest() -> TestResult {
    let _permit = cpu_permit().await?;
    let outcome = run("console.log(typeof Deno);", 64, Duration::from_secs(5)).await?;
    let GuestOutcome::Completed { output } = outcome else {
        return Err(format!("expected Completed, got {outcome:?}").into());
    };
    assert_eq!(output.text(), "undefined\n");
    Ok(())
}

#[tokio::test]
async fn infinite_loop_is_killed_by_timeout() -> TestResult {
    let _permit = cpu_permit().await?;
    let start = std::time::Instant::now();
    let outcome = run("while(true) {}", 64, Duration::from_secs(1)).await?;
    assert!(matches!(outcome, GuestOutcome::TimedOut { .. }));
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "took too long: {:?}",
        start.elapsed()
    );
    Ok(())
}

#[tokio::test]
async fn infinite_loop_is_killed_by_explicit_cancel_before_its_timeout() -> TestResult {
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    let handle = tokio::spawn(async move {
        let _permit = cpu_permit().await?;
        let outcome = run_js("while(true) {}".to_string(), 64, Duration::from_secs(60), cancel_clone).await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(outcome)
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(10), handle)
        .await??
        .map_err(|e| e.to_string())?;
    assert!(matches!(outcome, GuestOutcome::Cancelled { .. }));
    Ok(())
}

#[tokio::test]
async fn thrown_exception_yields_failed_outcome_with_error_message() -> TestResult {
    let _permit = cpu_permit().await?;
    let outcome = run("throw new Error('boom');", 64, Duration::from_secs(5)).await?;
    let GuestOutcome::Failed { error, .. } = outcome else {
        return Err(format!("expected Failed, got {outcome:?}").into());
    };
    assert!(error.contains("boom"), "error: {error}");
    Ok(())
}

#[tokio::test]
async fn console_error_is_captured_on_stderr_stream() -> TestResult {
    let _permit = cpu_permit().await?;
    let outcome = run("console.error('trouble');", 64, Duration::from_secs(5)).await?;
    let GuestOutcome::Completed { output } = outcome else {
        return Err(format!("expected Completed, got {outcome:?}").into());
    };
    assert_eq!(output.lines, vec![(OutputStream::Stderr, "trouble\n".to_string())]);
    Ok(())
}

#[tokio::test]
async fn one_shot_run_fails_instead_of_aborting_the_process_at_the_heap_limit() -> TestResult {
    let _permit = cpu_permit().await?;
    let outcome = run(
        "const a = []; while(true) { a.push(new Array(100000).fill(1)); }",
        16,
        Duration::from_secs(30),
    )
    .await?;
    let GuestOutcome::Failed { error, .. } = outcome else {
        return Err(format!("expected Failed, got {outcome:?}").into());
    };
    assert!(error.contains("memory limit"), "error: {error}");
    Ok(())
}
