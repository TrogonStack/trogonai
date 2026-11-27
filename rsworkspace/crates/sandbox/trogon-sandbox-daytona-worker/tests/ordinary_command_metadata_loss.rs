//! A finished command's completion lives in files the bridge wrote, not in the toolbox's session records, so losing
//! the records does not lose the result. Needs a Linux host and the two `SANDBOX_DAYTONA_COMMAND_SUPERVISOR_*`
//! variables; without them the test returns at once.

mod guest_support;

use guest_support::*;
use serde_json::json;

#[tokio::test]
async fn a_configured_completion_survives_the_loss_of_session_records() -> TestResult {
    let guest = Guest::start().await?;
    let Some(exec) = guest.exec_with_supervisor()? else {
        return Ok(());
    };
    let request = run_request(&["/bin/sh", "-c", "echo done; exit 4"], |wire| {
        wire["detached"] = json!(true)
    })?;
    let id = start(&guest, &exec, &request).await?;
    assert!(guest.settle().await, "the bridge never ended");
    guest.lose_sessions().await;
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.exit_code, 4);
    assert_eq!(output.stdout, b"done\n");
    Ok(())
}
