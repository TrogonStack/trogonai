mod command_deadline_support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use command_deadline_support::{Reply, Server, TestResult, exec, file_path, lease_sandbox, run_request};
use serde_json::json;
use trogon_sandbox_contract::{ListProcessesResponse, RunCommandResponse};

#[tokio::test]
async fn foreground_reads_final_output_after_native_group_end_and_retires_staging() -> TestResult {
    let ended = Arc::new(AtomicBool::new(false));
    let cleaned = Arc::new(AtomicBool::new(false));
    let directory = Arc::new(Mutex::new(None::<String>));
    let (native, cleanup, stage) = (ended.clone(), cleaned.clone(), directory.clone());
    let server = Server::start(move |request| {
        if request.path == "/process.Process/Start" {
            let args = request.args();
            let arg = |index: usize| args.get(index).cloned().unwrap_or_default();
            let script = arg(6);
            if script.contains("nohup setsid") {
                *stage.lock().unwrap_or_else(PoisonError::into_inner) = Some(arg(8));
                return Reply::process(b"123 456\n", 0);
            }
            if script.contains("/proc/") {
                native.store(true, Ordering::SeqCst);
                return Reply::process(b"", 44);
            }
            if script == "rm -rf -- \"$1\"" {
                let staged = stage.lock().unwrap_or_else(PoisonError::into_inner).clone();
                // Cleanup must follow the native end and target the staging directory.
                cleanup.store(
                    native.load(Ordering::SeqCst) && staged == Some(arg(8)),
                    Ordering::SeqCst,
                );
                return Reply::process(b"", 0);
            }
            return Reply::process(b"early", 7);
        }
        if request.path.starts_with("/files?") {
            let path = file_path(&request);
            let body: &[u8] = match path {
                _ if path.ends_with("/exit") => b"7\n",
                _ if path.ends_with("/out") && native.load(Ordering::SeqCst) => b"earlylate",
                _ if path.ends_with("/out") => b"early",
                _ => b"",
            };
            return Reply::Complete(200, body.to_vec());
        }
        Reply::json(404, json!({"message": "unknown fixture operation"}))
    })
    .await?;
    let sandbox = lease_sandbox(&server.url, "foreground-final-output")?;
    let exec = exec()?;
    let request = run_request(json!({"lease_id": "final-output", "command": ["finish"], "timeout_seconds": 60}))?;
    let response = exec.handle_run_command(&sandbox, &request).await;
    let inventory = exec.handle_list_processes(&sandbox, "final-output").await;
    server.shutdown().await;
    assert!(
        ended.load(Ordering::SeqCst)
            && cleaned.load(Ordering::SeqCst)
            && matches!(inventory, ListProcessesResponse::Processes { ref processes } if processes.is_empty()),
        "verified foreground completion must retire staging and inventory: ended={}, cleaned={}, inventory={inventory:?}",
        ended.load(Ordering::SeqCst),
        cleaned.load(Ordering::SeqCst)
    );
    assert!(
        matches!(response, RunCommandResponse::Completed(ref output)
        if output.exit_code == 7 && !output.timed_out.is_timed_out() && output.stdout == b"earlylate"),
        "foreground returned output captured before native group end: {response:?}"
    );
    Ok(())
}
