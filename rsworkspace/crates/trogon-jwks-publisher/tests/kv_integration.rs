//! NATS integration tests require a running NATS server with JetStream enabled.

use trogon_jwks_publisher::jwks::Jwks;
use trogon_jwks_publisher::publishers::kv::{open_kv_store, publish_jwks_to_kv};

#[tokio::test]
#[ignore = "requires NATS JetStream at NATS_URL"]
async fn publishes_jwks_to_kv_bucket() {
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let client = async_nats::connect(&nats_url).await.expect("connect");
    let js = async_nats::jetstream::new(client);
    let store = open_kv_store(&js, true).await.expect("open kv");

    let jwks = Jwks { keys: vec![] };
    publish_jwks_to_kv(&store, &jwks).await.expect("publish");
}
