use std::time::Duration;

use serde_json::json;

use crate::test_support::TlsJetStreamCluster;
use crate::{DottedNatsToken, NatsToken, connect_once_with_inbox_prefix, connect_with_inbox_prefix};

#[tokio::test]
async fn native_connectors_honor_the_only_reply_namespace_the_role_may_subscribe() {
    let identity = NatsToken::new("scoped-reader").expect("fixture identity");
    let prefix = DottedNatsToken::new("_INBOX.egress.registration_scope").expect("scoped reply prefix");
    let cluster = TlsJetStreamCluster::start(&[(
        identity.clone(),
        json!({
            "publish": ["$JS.API.INFO"], "subscribe": [format!("{prefix}.>")]
        }),
    )])
    .await;
    let config = cluster.config(&identity).expect("verified mapped reader certificate");
    let once = connect_once_with_inbox_prefix(&config, config.auth.clone(), Duration::from_secs(5), &prefix)
        .await
        .expect("native authenticated one-shot connector");
    tokio::time::timeout(Duration::from_secs(2), async_nats::jetstream::new(once).query_account())
        .await
        .expect("one-shot connector must receive the actual scoped reply")
        .expect("one-shot request succeeds under exact reply ACL");
    let background = connect_with_inbox_prefix(&config, Duration::from_secs(5), &prefix)
        .await
        .expect("native authenticated reconnecting connector");
    tokio::time::timeout(
        Duration::from_secs(2),
        async_nats::jetstream::new(background).query_account(),
    )
    .await
    .expect("background connector must receive the actual scoped reply")
    .expect("background request succeeds under exact reply ACL");
}
