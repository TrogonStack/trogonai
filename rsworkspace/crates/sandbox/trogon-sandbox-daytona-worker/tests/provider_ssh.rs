//! Minting SSH access through Daytona's SSH gateway: the endpoint, the validity window and every way a mint fails.

mod support;

use serde_json::json;
use support::{Fake, SANDBOX, TestResult};
use trogon_sandbox_contract::{LeaseError, SshAccessResponse};

fn route() -> String {
    format!("/sandbox/{SANDBOX}/ssh-access")
}

fn grant(command: &str, expires_at: &str) -> serde_json::Value {
    json!({"token": "tok-1", "sshCommand": command, "expiresAt": expires_at})
}

#[tokio::test]
async fn an_access_is_minted_for_the_requested_validity_rounded_up_to_whole_minutes() -> TestResult {
    let fake = Fake::start().await?;
    fake.json(
        "POST",
        &route(),
        200,
        grant("ssh -p 2222 tok-1@gateway.example", "2026-01-01T00:00:00Z"),
    );
    let provider = fake.provider()?;

    let response = provider.ssh_access_for_sandbox(SANDBOX, Some(541)).await;

    let SshAccessResponse::Available(access) = response else {
        return Err(format!("expected access, got {response:?}").into());
    };
    assert_eq!(access.host, "gateway.example");
    assert_eq!(access.port, 2222);
    assert_eq!(access.user, "tok-1");
    assert_eq!(access.expires_at, Some(1_767_225_600));
    assert_eq!(fake.calls("POST", &route()).len(), 1);
    assert!(
        fake.calls("POST", &route())
            .iter()
            .all(|call| call.query.contains("expiresInMinutes=10"))
    );
    Ok(())
}

#[tokio::test]
async fn an_access_without_a_validity_lasts_an_hour() -> TestResult {
    let fake = Fake::start().await?;
    fake.json(
        "POST",
        &route(),
        200,
        grant("ssh tok-1@gateway.example", "2026-01-01T00:00:00Z"),
    );
    let provider = fake.provider()?;

    let response = provider.ssh_access_for_sandbox(SANDBOX, None).await;

    let SshAccessResponse::Available(access) = response else {
        return Err(format!("expected access, got {response:?}").into());
    };
    assert_eq!(access.port, 22, "an ssh command without a port uses the default");
    assert!(
        fake.calls("POST", &route())
            .iter()
            .all(|call| call.query.contains("expiresInMinutes=60"))
    );
    Ok(())
}

#[tokio::test]
async fn a_validity_under_a_minute_is_one_minute() -> TestResult {
    let fake = Fake::start().await?;
    fake.json(
        "POST",
        &route(),
        200,
        grant("ssh tok-1@gateway.example", "2026-01-01T00:00:00Z"),
    );
    let provider = fake.provider()?;

    provider.ssh_access_for_sandbox(SANDBOX, Some(1)).await;

    assert!(
        fake.calls("POST", &route())
            .iter()
            .all(|call| call.query.contains("expiresInMinutes=1"))
    );
    Ok(())
}

#[tokio::test]
async fn a_sandbox_daytona_does_not_know_is_denied() -> TestResult {
    let fake = Fake::start().await?;
    fake.json("POST", &route(), 404, json!({"message": "not found"}));
    let provider = fake.provider()?;

    let response = provider.ssh_access_for_sandbox(SANDBOX, None).await;

    assert!(
        matches!(response, SshAccessResponse::Denied(LeaseError::Backend { .. })),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_grant_that_names_no_host_is_denied() -> TestResult {
    let fake = Fake::start().await?;
    fake.json("POST", &route(), 200, grant("ssh -p 2222", "2026-01-01T00:00:00Z"));
    let provider = fake.provider()?;

    let response = provider.ssh_access_for_sandbox(SANDBOX, None).await;

    assert!(
        matches!(response, SshAccessResponse::Denied(LeaseError::Backend { .. })),
        "{response:?}"
    );
    Ok(())
}
