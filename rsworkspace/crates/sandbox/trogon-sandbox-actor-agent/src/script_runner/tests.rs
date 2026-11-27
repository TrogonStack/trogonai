use std::error::Error;

use super::*;

#[tokio::test]
async fn a_zero_exit_is_success() -> Result<(), Box<dyn Error>> {
    assert_eq!(ProcessScriptRunner.run("sh", "exit 0").await?, ScriptOutcome::Succeeded);
    Ok(())
}

#[tokio::test]
async fn a_non_zero_exit_keeps_its_code_and_trimmed_stderr() -> Result<(), Box<dyn Error>> {
    let outcome = ProcessScriptRunner.run("sh", "echo '  nope  ' >&2; exit 3").await?;
    assert_eq!(
        outcome,
        ScriptOutcome::Failed {
            code: 3,
            stderr: "nope".to_string()
        }
    );
    Ok(())
}

#[tokio::test]
async fn a_signal_death_reports_a_synthetic_code() -> Result<(), Box<dyn Error>> {
    let outcome = ProcessScriptRunner.run("sh", "kill -9 $$").await?;
    assert!(matches!(outcome, ScriptOutcome::Failed { code: -1, .. }), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn a_shell_that_cannot_start_is_an_error() {
    let outcome = ProcessScriptRunner.run("/definitely/not/a/shell", "true").await;
    assert!(matches!(outcome, Err(LeaseError::Backend { .. })));
}
