use trogon_channel::{ChannelStore, ChannelStoreError, PrincipalId, SafeToken};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_std::env::InMemoryEnv;

use super::{config::BridgeConfig, seed_principals};

fn config(users: &str) -> BridgeConfig {
    let env = InMemoryEnv::new();
    env.set("TELEGRAM_BOT_TOKEN", "fixture-token");
    env.set("TELEGRAM_BOT_ACCOUNT", "fixturebot");
    env.set("CHANNEL_PREFIX", "seedtest");
    env.set("CHANNEL_SEED_TELEGRAM_USERS", users);
    BridgeConfig::from_env(&env).expect("bridge config")
}

#[tokio::test]
async fn configured_principals_resolve_to_the_same_endpoints_after_restart() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let config = config("42,7,42");
    let store = ChannelStore::ensure(&js, &config.channel_prefix)
        .await
        .expect("channel store");
    seed_principals(&store, &config).await.expect("initial seeding");
    seed_principals(&store, &config).await.expect("restart seeding");
    for user in [42_i64, 7] {
        let endpoint = config.account.endpoint_for(&SafeToken::from(user));
        assert_eq!(
            store.principal_for(&endpoint).await.expect("principal lookup"),
            Some(PrincipalId::new(format!("telegram-{user}")).expect("principal id"))
        );
        assert!(
            store
                .conversation_for(&endpoint)
                .await
                .expect("conversation lookup")
                .is_none()
        );
    }
    let unknown = config.account.endpoint_for(&SafeToken::from(8_i64));
    assert!(
        store
            .principal_for(&unknown)
            .await
            .expect("unknown principal")
            .is_none()
    );
}

#[tokio::test]
async fn empty_seed_configuration_does_not_touch_storage_and_write_failure_propagates() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let empty = config("");
    let store = ChannelStore::ensure(&js, &empty.channel_prefix)
        .await
        .expect("channel store");
    js.delete_key_value("channel_principals_seedtest")
        .await
        .expect("remove principal bucket");
    seed_principals(&store, &empty)
        .await
        .expect("empty seed does not access missing bucket");
    let error = seed_principals(&store, &config("42"))
        .await
        .expect_err("seed write must fail");
    assert!(error.downcast_ref::<ChannelStoreError>().is_some());
}
