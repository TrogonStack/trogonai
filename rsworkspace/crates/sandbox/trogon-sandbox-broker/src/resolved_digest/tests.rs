use std::error::Error;

use async_nats::jetstream;
use trogon_sandbox_contract::{Digest, LeaseId};

use super::*;
use crate::test_support::LocalNats;

type TestResult = Result<(), Box<dyn Error>>;

fn digest() -> Result<Digest, Box<dyn Error>> {
    let hex = "ab".repeat(32);
    Ok(Digest::new(format!("sha256:{hex}"))?)
}

async fn exercise(store: &dyn ResolvedDigestStore) -> TestResult {
    let lease = LeaseId::new("lease:one/a.b");
    assert!(store.get(&lease).await?.is_none());
    store.record(&lease, &digest()?).await?;
    assert_eq!(store.get(&lease).await?, Some(digest()?));
    store.clear(&lease).await?;
    assert!(store.get(&lease).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn in_memory_store_round_trips() -> TestResult {
    exercise(&InMemoryResolvedDigestStore::new()).await
}

#[tokio::test]
async fn nats_store_round_trips() -> TestResult {
    let server = LocalNats::start().await;
    let store = NatsKvResolvedDigestStore::new(jetstream::new(server.client.clone())).await?;
    exercise(&store).await
}
