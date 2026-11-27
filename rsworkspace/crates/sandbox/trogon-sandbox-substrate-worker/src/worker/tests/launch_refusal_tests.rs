use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::routing::post;
use serde_json::json;
use trogon_sandbox_actor_agent::EXEC_PATH;
use trogon_sandbox_contract::{LeaseError, LeaseId, RunCommandResponse};
use trogon_sandbox_protocol::subjects;

use super::support::{HELD, MISROUTED, Rig, TestResult};
use crate::agent::tests::ndjson;

#[tokio::test]
async fn unsupported_launch_cancellation_is_refused_before_native_api_calls() -> TestResult {
    let calls = Arc::new(AtomicUsize::new(0));
    let rig = Rig::start(Router::new().route(
        EXEC_PATH,
        post({
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { ndjson("{\"outcome\":\"completed\",\"exit_code\":0,\"stdout\":[],\"stderr\":[]}\n") }
            }
        }),
    ))
    .await?;
    let subject = subjects::provider_run_command(trogon_sandbox_contract::ProviderKind::Substrate, &LeaseId::new(HELD));
    let ordinary = json!({"lease_id": HELD, "command": ["true"], "timeout_seconds": 1});

    let first: RunCommandResponse = rig.ask(subject.clone(), &ordinary).await?;
    assert!(matches!(first, RunCommandResponse::Completed(ref output) if output.exit_code == 0));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "an ordinary command reaches the agent");

    let reads_before = rig.control.get_count();
    let strong = json!({
        "lease_id": HELD,
        "command": ["true"],
        "timeout_seconds": 1,
        "require_launch_cancellation": true,
        "launch_key": "unsupported-strong-request",
    });
    let refused: RunCommandResponse = rig.ask(subject, &strong).await?;

    assert!(matches!(
        refused,
        RunCommandResponse::Denied(LeaseError::InvalidSpec { ref reason }) if reason.contains("command launch cancellation")
    ));
    assert_eq!(
        rig.control.get_count(),
        reads_before,
        "the control plane is not consulted"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the agent is not called");
    assert!(rig.provider.active_lease(&LeaseId::new(MISROUTED)).await.is_some());
    rig.finish().await
}
