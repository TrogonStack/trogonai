#[allow(
    dead_code,
    reason = "shared support; this binary uses only the running worker and its fake"
)]
mod nats_support;
mod support;

use nats_support::Running;
use serde_json::json;
use support::{SANDBOX, TestResult, request_with};

#[tokio::test(flavor = "multi_thread")]
async fn unknown_native_options_get_a_native_rejection_reply() -> TestResult {
    let mut running = Running::start().await?;
    let request = request_with(|wire| wire["native"] = json!({"daytona": {"typo": 1}}))?;
    let reply = running
        .ask(
            "sandbox.provider.daytona.lease",
            &json!({"lease_id": "lease-1", "request": request}),
        )
        .await?;
    assert!(
        reply["native_error"].as_str().is_some_and(|reason| !reason.is_empty()),
        "{reply}"
    );
    assert!(reply.get("provider_error").is_none(), "{reply}");
    assert_eq!(running.fake.count("POST", "/sandbox"), 0);
    assert_eq!(running.fake.count("DELETE", &format!("/sandbox/{SANDBOX}")), 0);
    assert_eq!(running.provider.active_lease_count().await, 0);
    running.stop().await
}
