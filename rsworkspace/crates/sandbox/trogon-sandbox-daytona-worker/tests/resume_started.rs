mod support;

use serde_json::json;
use support::*;
use trogon_sandbox_contract::SandboxFreezeResponse;
use trogon_sandbox_protocol::Provider;

fn sandbox_route() -> String {
    format!("/sandbox/{SANDBOX}")
}

#[tokio::test]
async fn resuming_a_started_sandbox_is_done_without_a_start_request() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    provider.lease(&request()?, &lease_id("l1")).await?;
    let response = provider.set_paused_for_lease(&lease_id("l1"), false).await;
    assert!(matches!(response, SandboxFreezeResponse::Done), "{response:?}");
    assert_eq!(fake.count("POST", &format!("{}/start", sandbox_route())), 0);
    assert_eq!(fake.count("POST", &format!("{}/stop", sandbox_route())), 0);
    Ok(())
}

#[tokio::test]
async fn a_second_resume_after_a_pause_and_resume_stays_a_no_op() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json("POST", &format!("{}/stop", sandbox_route()), 200, json!({}));
    fake.json("POST", &format!("{}/start", sandbox_route()), 200, json!({}));
    let provider = fake.provider()?;
    provider.lease(&request()?, &lease_id("l1")).await?;
    fake.script(
        "GET",
        &sandbox_route(),
        vec![Reply::json(200, record("started")), Reply::json(200, record("stopped"))],
    );
    assert!(matches!(
        provider.set_paused_for_lease(&lease_id("l1"), true).await,
        SandboxFreezeResponse::Done
    ));
    fake.script(
        "GET",
        &sandbox_route(),
        vec![Reply::json(200, record("stopped")), Reply::json(200, record("started"))],
    );
    assert!(matches!(
        provider.set_paused_for_lease(&lease_id("l1"), false).await,
        SandboxFreezeResponse::Done
    ));
    let starts = fake.count("POST", &format!("{}/start", sandbox_route()));
    assert_eq!(starts, 1);
    assert!(matches!(
        provider.set_paused_for_lease(&lease_id("l1"), false).await,
        SandboxFreezeResponse::Done
    ));
    assert_eq!(fake.count("POST", &format!("{}/start", sandbox_route())), starts);
    Ok(())
}
