use trogon_nats::test_support::CoreTestServer;
use trogon_std::env::InMemoryEnv;

use super::*;
use crate::constants::{
    ENV_GATEWAY_AUDIT_INSTANCE, ENV_GATEWAY_AUDIT_PROJECT, ENV_GATEWAY_AUDIT_PUBLISH, ENV_GATEWAY_AUDIT_TENANT,
};

fn configured() -> InMemoryEnv {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_AUDIT_PUBLISH, "true");
    env.set(ENV_GATEWAY_AUDIT_TENANT, "acme");
    env.set(ENV_GATEWAY_AUDIT_PROJECT, "payments");
    env.set(ENV_GATEWAY_AUDIT_INSTANCE, "gw-1");
    env
}

#[tokio::test]
async fn no_writer_is_built_while_audit_publishing_is_off() {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await.expect("connect");
    let env = configured();
    env.remove(ENV_GATEWAY_AUDIT_PUBLISH);
    assert!(jetstream_audit_from_env(&env, &client).expect("settings").is_none());
}

#[tokio::test]
async fn settings_that_cannot_build_a_writer_stop_the_gateway_at_start() {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await.expect("connect");
    let env = configured();
    env.remove(ENV_GATEWAY_AUDIT_TENANT);
    assert_eq!(
        jetstream_audit_from_env(&env, &client).err(),
        Some(PlatformAuditConfigError::Missing(ENV_GATEWAY_AUDIT_TENANT))
    );
}
