//! The ADR#0058 publish gate, against a live JetStream and the real component.
//!
//! What matters here is not that a good component publishes. It is that a bad
//! one leaves the bucket exactly as it found it: a gate that reports a failure
//! and stores the module anyway is not a gate, and every host downstream would
//! be fetching something no suite ever passed.

#![cfg(not(coverage))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::PathBuf;

use trogon_decider_publish::{PublishError, PublishRequest, publish};
use trogon_decider_test::Suite;
use trogon_decider_test::conformance::OutputFormat;
use trogon_nats::test_support::JetStreamTestServer;

const BUCKET: &str = "PUBLISH_GATE_TEST";
const MODULE_REFERENCE: &str = "scheduler.schedules@0.1.0";

fn schedules_wasm() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-unknown-unknown/release/trogon_schedules_decider.wasm");
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "build trogon_schedules_decider.wasm for wasm32-unknown-unknown first (expected {}): {error}",
            path.display()
        )
    })
}

fn schedules_suite() -> Suite {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../trogon-decider-test/schedules.yaml");
    Suite::from_yaml(&std::fs::read_to_string(path).expect("the checked-in suite is readable")).expect("it parses")
}

async fn bucket_holds(js: &async_nats::jetstream::Context, key: &str) -> bool {
    let Ok(store) = js.get_object_store(BUCKET).await else {
        return false;
    };
    store.info(key).await.is_ok()
}

#[tokio::test]
async fn a_conformant_component_is_published_under_the_reference_it_declares() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let reference = publish(
        &js,
        &PublishRequest {
            component: &schedules_wasm(),
            suite: &schedules_suite(),
            bucket: BUCKET,
            format: OutputFormat::Tap,
        },
    )
    .await
    .expect("the checked-in component passes its checked-in suite");

    assert_eq!(reference.to_string(), MODULE_REFERENCE);
    assert!(
        bucket_holds(&js, &reference.object_key()).await,
        "the reference a host is configured with has to be the key the bytes actually landed under"
    );
}

#[tokio::test]
async fn a_component_whose_suite_fails_never_reaches_the_bucket() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    // A suite naming a module this component is not: the cheapest way to make
    // the gate fail without shipping a deliberately broken component.
    let suite = Suite::from_yaml("suite: not.a.real.module\nscenarios: []\n").expect("it parses");

    let error = publish(
        &js,
        &PublishRequest {
            component: &schedules_wasm(),
            suite: &suite,
            bucket: BUCKET,
            format: OutputFormat::Tap,
        },
    )
    .await
    .expect_err("the suite does not describe this component");

    assert!(matches!(error, PublishError::Suite { .. }), "{error}");
    assert!(
        js.get_object_store(BUCKET).await.is_err(),
        "a failed gate must not even provision the bucket, or the next 'module not found' reads like a \
         typo in a reference rather than a publish that was refused"
    );
}

#[tokio::test]
async fn something_that_is_not_a_component_never_reaches_the_bucket() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let error = publish(
        &js,
        &PublishRequest {
            component: b"\0asm not really",
            suite: &schedules_suite(),
            bucket: BUCKET,
            format: OutputFormat::Tap,
        },
    )
    .await
    .expect_err("those bytes are not a decider component");

    assert!(
        matches!(error, PublishError::Suite { .. } | PublishError::Load(_)),
        "{error}"
    );
    assert!(js.get_object_store(BUCKET).await.is_err(), "nothing was published");
}

#[tokio::test]
async fn republishing_the_same_reference_replaces_it_rather_than_failing() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let request = PublishRequest {
        component: &schedules_wasm(),
        suite: &schedules_suite(),
        bucket: BUCKET,
        format: OutputFormat::Tap,
    };

    publish(&js, &request).await.expect("the first publish lands");
    let reference = publish(&js, &request).await.expect(
        "a publisher that had to delete first would leave the bucket briefly missing a module every \
         host is configured to fetch",
    );

    assert!(bucket_holds(&js, &reference.object_key()).await);
}
