use std::process::Output;

use tokio::process::Command;
use trogon_nats::test_support::JetStreamTestServer;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const BIN: &str = env!("CARGO_BIN_EXE_trogon-sandbox-admin");

async fn run(args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new(BIN).args(args).env_remove("NATS_URL").output().await?)
}

#[tokio::test]
async fn help_lists_the_command_groups() -> TestResult {
    let output = run(&["--help"]).await?;
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for group in ["api-key", "tag", "worker", "credential", "lease", "substrate"] {
        assert!(help.contains(group), "{group} missing from {help}");
    }
    Ok(())
}

#[tokio::test]
async fn a_malformed_tenant_is_rejected_before_connecting() -> TestResult {
    let output = run(&["--nats-url", "127.0.0.1:1", "image-policy", "show", "--tenant", ""]).await?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("tenant"));
    Ok(())
}

#[tokio::test]
async fn an_unreachable_server_fails_the_command() -> TestResult {
    let output = run(&["--nats-url", "127.0.0.1:1", "worker", "list"]).await?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("NATS"));
    Ok(())
}

#[tokio::test]
async fn credential_commands_need_a_key() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let output = run(&["--nats-url", server.address(), "credential", "list", "docker"]).await?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--kms-key"));
    Ok(())
}

#[tokio::test]
async fn policies_and_tags_round_trip_through_the_binary() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let url = server.address();

    let set = run(&[
        "--nats-url",
        url,
        "image-policy",
        "set",
        "--tenant",
        "acme",
        "--pattern",
        "registry.example/team/*",
    ])
    .await?;
    assert!(set.status.success(), "{}", String::from_utf8_lossy(&set.stderr));

    let show = run(&["--nats-url", url, "image-policy", "show", "--tenant", "acme"]).await?;
    assert!(show.status.success());
    assert!(String::from_utf8_lossy(&show.stdout).contains("registry.example/team/*"));

    let digest = format!("sha256:{}", "cd".repeat(32));
    let tag = run(&["--nats-url", url, "tag", "set", "acme", "stable", &digest]).await?;
    assert!(tag.status.success(), "{}", String::from_utf8_lossy(&tag.stderr));

    let list = run(&["--nats-url", url, "tag", "list", "acme"]).await?;
    assert!(String::from_utf8_lossy(&list.stdout).contains(&digest));

    let missing = run(&["--nats-url", url, "tag", "get", "acme", "other"]).await?;
    assert!(!missing.status.success());
    Ok(())
}

#[tokio::test]
async fn minting_prints_the_token_on_stdout_only() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let output = run(&[
        "--nats-url",
        server.address(),
        "api-key",
        "mint",
        "--tenant",
        "acme",
        "--service-account",
        "ci",
        "--name",
        "pipeline",
    ])
    .await?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim().len(), 64);
    assert!(String::from_utf8_lossy(&output.stderr).contains("store this token securely"));
    Ok(())
}
