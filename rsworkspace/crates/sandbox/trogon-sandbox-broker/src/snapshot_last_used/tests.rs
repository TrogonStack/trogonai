use std::str::FromStr;
use std::time::{Duration, SystemTime};

use trogon_sandbox_contract::Digest;

use super::*;

fn digest(hex: &str) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::from_str(&format!("sha256:{hex:0<64}"))?)
}
#[tokio::test]
async fn in_memory_store_round_trips_and_overwrites() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemorySnapshotLastUsedStore::new();
    let d = digest("a")?;
    assert_eq!(store.get(&d).await?, None);

    let first = SystemTime::now() - Duration::from_secs(3600);
    store.touch(&d, first).await?;
    assert_eq!(store.get(&d).await?, Some(first));

    let second = SystemTime::now();
    store.touch(&d, second).await?;
    assert_eq!(store.get(&d).await?, Some(second));
    Ok(())
}
#[tokio::test]
async fn in_memory_store_tracks_digests_independently() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemorySnapshotLastUsedStore::new();
    let a = digest("a")?;
    let b = digest("b")?;

    let at = SystemTime::now();
    store.touch(&a, at).await?;

    assert_eq!(store.get(&a).await?, Some(at));
    assert_eq!(store.get(&b).await?, None);
    Ok(())
}
