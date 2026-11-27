use async_nats::jetstream::kv;
use trogon_nats::test_support::JetStreamTestServer;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn a_bucket_that_can_be_neither_loaded_nor_created_is_unavailable() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let config = kv::Config {
        bucket: "not a bucket name".into(),
        ..Default::default()
    };
    let refused = open_custody_bucket(&server.jetstream().await, config).await;
    assert!(matches!(refused, Err(CustodyBucketError::Unavailable { .. })));
    Ok(())
}

#[tokio::test]
async fn an_absent_bucket_is_created_and_then_reopened() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let config = || kv::Config {
        bucket: "custody-fixture".into(),
        ..Default::default()
    };
    open_custody_bucket(&context, config()).await?;
    open_custody_bucket(&context, config()).await?;
    Ok(())
}

#[tokio::test]
async fn a_bucket_that_could_expire_records_is_refused() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let config = kv::Config {
        bucket: "expiring".into(),
        max_age: std::time::Duration::from_secs(60),
        ..Default::default()
    };
    let refused = open_custody_bucket(&server.jetstream().await, config).await;
    assert!(matches!(refused, Err(CustodyBucketError::NotDurable)));
    Ok(())
}
