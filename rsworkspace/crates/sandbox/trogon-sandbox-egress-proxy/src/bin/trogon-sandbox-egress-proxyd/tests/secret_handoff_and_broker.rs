//! Startup with a secret handoff and with the credential broker, which need an inherited descriptor.

use super::*;

#[tokio::test]
async fn a_secret_handoff_is_loaded_before_serving() -> TestResult {
    let wire = RegistryHandoff::from_registry(&SentinelRegistry::new())?;
    let fd = secret_pipe(wire.expose_wire().as_bytes())?.to_string();
    serves(&[(SECRETS_FD_ENV_VAR, &fd)], r#""allow_all""#).await
}

#[tokio::test]
async fn a_secret_handoff_that_does_not_decode_stops_the_proxy() -> TestResult {
    let _logs = sink_logs();
    let fd = secret_pipe(b"not a handoff")?.to_string();
    let config = config_with(&[(SECRETS_FD_ENV_VAR, &fd)], r#""allow_all""#)?;
    assert!(run(config).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_proxy_with_a_broker_decider_and_violation_reporting_serves() -> TestResult {
    let server = trogon_nats::test_support::JetStreamTestServer::start().await;
    let url = format!("nats://{}", server.address());
    let wire = RegistryHandoff::from_registry(&SentinelRegistry::new())?;
    let fd = secret_pipe(wire.expose_wire().as_bytes())?.to_string();
    let policy = policy_json_with_rule("api.example.com", TRANSFORM_ACTION_JSON);
    serves(
        &[
            (CREDENTIAL_BROKER_NATS_URL_ENV_VAR, &url),
            (CREDENTIAL_BROKER_TENANT_ID_ENV_VAR, "tenant-1"),
            (VIOLATION_NATS_URL_ENV_VAR, &url),
            (VIOLATION_LEASE_ID_ENV_VAR, "lease-1"),
            (DECIDER_TIMEOUT_MS_ENV_VAR, "1000"),
            (SECRETS_FD_ENV_VAR, &fd),
        ],
        &policy,
    )
    .await
}

#[tokio::test]
async fn a_fenced_broker_connection_serves() -> TestResult {
    let server = trogon_nats::test_support::JetStreamTestServer::start().await;
    let url = format!("nats://{}", server.address());
    let policy = policy_json_with_rule("api.example.com", TRANSFORM_ACTION_JSON);
    serves(
        &[
            (CREDENTIAL_BROKER_NATS_URL_ENV_VAR, &url),
            (CREDENTIAL_BROKER_TENANT_ID_ENV_VAR, "tenant-1"),
            (CREDENTIAL_BROKER_LEASE_ID_ENV_VAR, "lease-1"),
            (CREDENTIAL_BROKER_POLICY_REVISION_ENV_VAR, "2"),
        ],
        &policy,
    )
    .await
}
