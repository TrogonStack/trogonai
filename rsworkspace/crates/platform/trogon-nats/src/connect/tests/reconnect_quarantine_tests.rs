use std::time::{Duration, Instant};

use async_nats::connection::State;
use serde_json::json;

use crate::test_support::TlsJetStreamCluster;
use crate::{DottedNatsToken, NatsToken, connect_once_with_reconnect_quarantine};

#[tokio::test]
async fn authenticated_initial_session_is_immediate_and_later_dials_wait_for_quarantine() {
    let identity = NatsToken::new("scoped-session").expect("identity");
    let prefix = DottedNatsToken::new("_INBOX.control.session").expect("reply scope");
    let cluster = TlsJetStreamCluster::start(&[(
        identity.clone(),
        json!({
            "publish": ["$JS.API.INFO"], "subscribe": [format!("{prefix}.>")],
        }),
    )])
    .await;
    let config = cluster.config(&identity).expect("mapped mutual TLS identity");
    let started = Instant::now();
    let client = tokio::time::timeout(
        Duration::from_secs(5),
        connect_once_with_reconnect_quarantine(
            &config,
            config.auth.clone(),
            Duration::from_secs(5),
            &prefix,
            Duration::from_secs(10),
        ),
    )
    .await
    .expect("initial session does not wait for reconnect quarantine")
    .expect("authenticated connection");
    assert!(started.elapsed() < Duration::from_secs(10));
    async_nats::jetstream::new(client.clone())
        .query_account()
        .await
        .expect("native scoped TLS request");
    let before = client.server_info();
    let reconnect = Instant::now();
    client.force_reconnect().await.expect("real disconnect injection");
    let premature = tokio::time::timeout(Duration::from_millis(300), async {
        loop {
            let after = client.server_info();
            if client.connection_state() == State::Connected
                && (after.server_id != before.server_id || after.client_id != before.client_id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        premature.is_err(),
        "a replacement authenticated transport must wait for quarantine"
    );
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let after = client.server_info();
            if client.connection_state() == State::Connected
                && (after.server_id != before.server_id || after.client_id != before.client_id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bounded quarantine permits later reconnect as a positive control");
    assert!(reconnect.elapsed() >= Duration::from_secs(10));
    async_nats::jetstream::new(client)
        .query_account()
        .await
        .expect("native scoped TLS request after quarantine");
}
