#![allow(clippy::expect_used)]

use std::time::Duration;

use async_nats::jetstream::stream::{Config, StorageType};
use trogon_nats::test_support::TlsJetStreamCluster;
use trogon_nats::{NatsToken, connect_once};

#[tokio::test]
async fn the_production_replication_requirement_has_real_verified_tls_peers() {
    let cluster = TlsJetStreamCluster::start(&[]).await;
    let config = cluster
        .config(&NatsToken::new("fixture-admin").expect("fixture identity"))
        .expect("administrator certificate");
    let client = connect_once(&config, config.auth.clone(), Duration::from_secs(5))
        .await
        .expect("verified authenticated TLS connection");
    let stream = async_nats::jetstream::new(client)
        .create_stream(Config {
            name: "registration_proof".into(),
            subjects: vec!["registration.proof".into()],
            num_replicas: 3,
            storage: StorageType::File,
            max_messages: 1,
            ..Config::default()
        })
        .await
        .expect("create actual three-replica stream");
    let info = stream.get_info().await.expect("broker stream metadata");
    assert_eq!(info.config.num_replicas, 3);
    assert_eq!(info.cluster.expect("actual replication peers").replicas.len(), 2);
}
