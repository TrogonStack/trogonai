#[cfg(feature = "spicedb")]
use std::error::Error as _;

use trogon_std::env::InMemoryEnv;

use super::*;

#[cfg(feature = "spicedb")]
#[tokio::test]
async fn a_closed_ingress_finishes_after_dispatching_its_final_envelope() {
    let prefix = a2a_nats::A2aPrefix::new("a2a").unwrap();
    let message = crate::gateway_test_support::event(async_nats::HeaderMap::new());
    let mut payloads = Vec::new();
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _capture = events.install(trogon_std::log_capture::LevelFilter::DEBUG);
    receive_ingress(
        futures::stream::iter([message]),
        &prefix,
        CancellationToken::new(),
        |message| {
            payloads.push(message.payload);
            std::future::ready(())
        },
    )
    .await;
    assert_eq!(payloads, [bytes::Bytes::from_static(b"event payload")]);
    assert!(
        events
            .events()
            .iter()
            .any(|event| event.message() == Some("gateway ingress NATS subscription closed"))
    );
}

#[cfg(feature = "spicedb")]
#[tokio::test]
async fn startup_preserves_enabled_policy_configuration_failures() {
    let server = trogon_nats::test_support::CoreTestServer::start().await;
    for setting in [
        ("A2A_GATEWAY_TIER1_SPICEDB_ENABLED", "true"),
        ("A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED", "true"),
        ("A2A_GATEWAY_AAUTH_MODE", "invalid"),
    ] {
        let env = InMemoryEnv::new();
        env.set(
            "AUTH_CALLOUT_SIGNING_SECRET",
            nkeys::KeyPair::new_account().seed().unwrap(),
        );
        env.set(setting.0, setting.1);
        let error = run_with_args(
            Args {
                nats_url: server.address().to_owned(),
                prefix: "a2a".to_owned(),
                queue_group: None,
            },
            &env,
        )
        .await
        .unwrap_err();
        match setting.0 {
            "A2A_GATEWAY_TIER1_SPICEDB_ENABLED" => assert!(matches!(error, RuntimeError::Tier1Config(_))),
            "A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED" => {
                assert!(matches!(error, RuntimeError::Tier1DeclarativeConfig(_)))
            }
            _ => assert!(matches!(error, RuntimeError::AAuthConfig(_))),
        }
        assert!(error.source().is_some());
    }
}

#[cfg(not(feature = "spicedb"))]
#[tokio::test(flavor = "current_thread")]
async fn run_with_args_resolves_config_and_returns_ok() {
    let env = InMemoryEnv::new();
    let args = Args {
        nats_url: "localhost:4222".to_string(),
        prefix: "a2a".to_string(),
        queue_group: None,
    };
    run_with_args(args, &env).await.expect("bootstrap config seam");
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_args_surfaces_config_error() {
    let env = InMemoryEnv::new();
    let args = Args {
        nats_url: "localhost:4222".to_string(),
        prefix: "bad prefix!".to_string(),
        queue_group: None,
    };
    let err = run_with_args(args, &env).await.unwrap_err();
    assert!(matches!(err, RuntimeError::Config(ConfigError::InvalidPrefix(_))));
}

#[cfg(feature = "spicedb")]
#[tokio::test]
async fn run_with_args_preserves_subscription_error_source() {
    let server = trogon_nats::test_support::CoreTestServer::start().await;
    let env = InMemoryEnv::new();
    env.set(
        "AUTH_CALLOUT_SIGNING_SECRET",
        nkeys::KeyPair::new_account().seed().expect("account signing seed"),
    );
    let args = Args {
        nats_url: server.address().to_owned(),
        prefix: "a2a".to_owned(),
        queue_group: Some("invalid queue".to_owned()),
    };

    let error = tokio::time::timeout(std::time::Duration::from_secs(10), run_with_args(args, &env))
        .await
        .expect("invalid subscription must terminate startup")
        .expect_err("invalid queue name must fail subscription");

    assert!(matches!(error, RuntimeError::Subscribe(_)));
    assert_eq!(error.to_string(), "gateway subscribe: invalid queue name");
    let source = error.source().expect("subscription failure must retain its source");
    let subscription = source
        .downcast_ref::<async_nats::client::SubscribeError>()
        .expect("source must preserve the SDK subscription error type");
    assert_eq!(
        subscription.kind(),
        async_nats::client::SubscribeErrorKind::InvalidQueueName
    );
}
