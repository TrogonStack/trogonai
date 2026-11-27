mod nats_common;

use trogon_sandbox_protocol::NatsConnectError;
use trogon_sandbox_protocol::nats_connect::connect;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the rendered message is the contract under test; typed variants are matched separately"
    )
)]
async fn an_unparseable_url_is_an_invalid_url_error() -> TestResult {
    let error = connect("not a url").await.err().ok_or("expected an error")?;
    assert!(matches!(error, NatsConnectError::InvalidUrl(_)));
    assert!(error.to_string().starts_with("invalid NATS URL"));
    Ok(())
}

#[tokio::test]
async fn an_unreachable_server_is_a_connect_error() -> TestResult {
    let error = connect("nats://127.0.0.1:1").await.err().ok_or("expected an error")?;
    assert!(matches!(error, NatsConnectError::Connect(_)));
    Ok(())
}

#[tokio::test]
async fn connects_without_credentials() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = connect(&server.url()).await?;
    client.flush().await?;
    Ok(())
}

#[tokio::test]
async fn userinfo_in_the_url_authenticates_as_user_and_password() -> TestResult {
    let server = nats_common::Server::start_with_config(
        "authorization { users = [ { user: \"alice\", password: \"s3cret\" } ] }\n",
    )
    .await;
    let url = server.url().replacen("nats://", "nats://alice:s3cret@", 1);
    let client = connect(&url).await?;
    client.flush().await?;

    let rejected = server.url().replacen("nats://", "nats://alice:wrong@", 1);
    assert!(connect(&rejected).await.is_err());
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the rendered message is the contract under test; typed variants are matched separately"
    )
)]
async fn a_user_without_a_password_is_refused() -> TestResult {
    let error = connect("nats://alice@127.0.0.1:4222")
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, NatsConnectError::MissingPassword(_)));
    assert!(error.to_string().contains("no password"));
    Ok(())
}

#[tokio::test]
async fn a_tls_url_requires_tls_and_reports_an_unreachable_server() -> TestResult {
    let error = connect("tls://127.0.0.1:1").await.err().ok_or("expected an error")?;
    assert!(matches!(error, NatsConnectError::Connect(_)));
    Ok(())
}
