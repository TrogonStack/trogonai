#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! Proves the default TLS path end to end against a live server.
//!
//! This binary links both rustls providers on purpose: the workspace `rustls`
//! enables `aws-lc-rs` and the dev-dependency adds `ring`. Nothing here
//! installs one, so a handshake only completes because
//! [`trogon_nats::connect`] installs the shared provider. Without it, rustls
//! panics in the connection task and no server `INFO` ever arrives.

use std::path::PathBuf;
use std::time::Duration;

use async_nats::ServerInfo;
use trogon_nats::test_support::TlsTestServer;
use trogon_nats::{NatsConfig, ServerInfoTimeoutError, connect, wait_for_server_info};
use trogon_std::env::InMemoryEnv;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(15);
const REJECTION_WINDOW: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

fn config(server: &TlsTestServer, files: &[(&str, PathBuf)]) -> NatsConfig {
    let env = InMemoryEnv::new();
    env.set("NATS_URL", server.address());
    for (setting, path) in files {
        env.set(*setting, path.to_str().expect("TLS test files live at UTF-8 paths"));
    }
    NatsConfig::from_env(&env)
}

async fn handshake(config: &NatsConfig, deadline: Duration) -> Result<ServerInfo, ServerInfoTimeoutError> {
    let client = connect(config, CONNECT_TIMEOUT).await.expect("TLS settings are valid");
    wait_for_server_info(&client, deadline, POLL_INTERVAL).await
}

#[tokio::test]
async fn default_settings_trust_a_server_through_nats_ca() {
    let server = TlsTestServer::start().await;
    let config = config(&server, &[("NATS_CA", server.ca())]);

    let info = handshake(&config, HANDSHAKE_DEADLINE)
        .await
        .expect("TLS handshake with a server the CA issued for");

    assert!(info.tls_required);
}

#[tokio::test]
async fn nats_cert_and_key_present_a_client_certificate() {
    let server = TlsTestServer::start_verifying_clients().await;
    let without_certificate = config(&server, &[("NATS_CA", server.ca())]);
    let with_certificate = config(
        &server,
        &[
            ("NATS_CA", server.ca()),
            ("NATS_CERT", server.client_cert()),
            ("NATS_KEY", server.client_key()),
        ],
    );

    assert!(
        handshake(&without_certificate, REJECTION_WINDOW).await.is_err(),
        "the server must turn away a client that presents no certificate"
    );
    let info = handshake(&with_certificate, HANDSHAKE_DEADLINE)
        .await
        .expect("TLS handshake presenting the client certificate");

    assert!(info.tls_required);
}
