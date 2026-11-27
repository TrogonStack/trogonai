mod command_deadline_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use command_deadline_support::{Reply, Server, TestResult, exec, lease_sandbox, run_request};
use serde_json::json;
use trogon_sandbox_contract::{LeaseError, RunCommandResponse};

#[tokio::test]
async fn a_successful_remote_bootstrap_with_bad_identity_is_not_pre_effect_invalid_spec() -> TestResult {
    let starts = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicBool::new(false));
    let (count, remote) = (starts.clone(), active.clone());
    let server = Server::start(move |request| {
        let bootstrap =
            request.path == "/process.Process/Start" && request.args().iter().any(|arg| arg.contains("nohup setsid"));
        if !bootstrap {
            return Reply::json(503, json!({"message": "receipt recovery is unavailable"}));
        }
        count.fetch_add(1, Ordering::SeqCst);
        remote.store(true, Ordering::SeqCst);
        Reply::process(b"invalid identity\n", 0)
    })
    .await?;
    let sandbox = lease_sandbox(&server.url, "original-sandbox")?;
    let request = run_request(json!({"lease_id": "bootstrap-lease", "command": ["sleep", "60"], "detached": true}))?;
    let response = exec()?.handle_run_command(&sandbox, &request).await;
    server.shutdown().await;
    assert_eq!(
        starts.load(Ordering::SeqCst),
        1,
        "the successful bootstrap reached the native API"
    );
    assert!(
        active.swap(false, Ordering::SeqCst),
        "the fixture accepted remote execution before returning the bad receipt"
    );
    assert!(
        matches!(response, RunCommandResponse::Denied(LeaseError::Backend { .. })),
        "a malformed after-effect receipt is an unknown execution outcome, never retryable pre-effect input: {response:?}"
    );
    Ok(())
}
