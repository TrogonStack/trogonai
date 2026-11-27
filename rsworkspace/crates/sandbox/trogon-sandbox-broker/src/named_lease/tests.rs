use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use trogon_sandbox_contract::{LeaseId, TenantId};

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

const TTL: Duration = Duration::from_secs(30);

fn tenant(value: &str) -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new(value)?)
}

#[tokio::test]
async fn a_fresh_claim_is_pending_until_confirmed() -> TestResult {
    let store = InMemoryNamedLeaseStore::new();
    let tenant = tenant("t")?;
    let first = LeaseId::new("first");
    let second = LeaseId::new("second");
    assert_eq!(store.claim(&tenant, "n", &first, TTL).await?, NameClaim::Claimed);
    assert!(matches!(
        store.claim(&tenant, "n", &second, TTL).await?,
        NameClaim::Held { pending: true, .. }
    ));
    assert!(store.confirm(&tenant, "n", &first).await?);
    assert!(matches!(
        store.claim(&tenant, "n", &second, TTL).await?,
        NameClaim::Held { pending: false, .. }
    ));
    assert!(!store.confirm(&tenant, "n", &second).await?);
    Ok(())
}

#[tokio::test]
async fn a_pending_claim_stops_blocking_after_its_ttl() -> TestResult {
    let store = InMemoryNamedLeaseStore::new();
    let tenant = tenant("t")?;
    let first = LeaseId::new("first");
    let second = LeaseId::new("second");
    store.claim(&tenant, "n", &first, Duration::from_millis(20)).await?;
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(matches!(
        store.claim(&tenant, "n", &second, TTL).await?,
        NameClaim::Held { pending: false, .. }
    ));
    assert!(store.replace_if_matches(&tenant, "n", &first, &second, TTL).await?);
    assert!(matches!(
        store.claim(&tenant, "n", &first, TTL).await?,
        NameClaim::Held { pending: true, .. }
    ));
    Ok(())
}

#[test]
fn stored_claims_keep_the_legacy_bare_lease_encoding() -> TestResult {
    let lease = LeaseId::new("01J0");
    let confirmed = StoredClaim::Confirmed(lease.clone()).encode()?;
    assert_eq!(&confirmed[..], br#""01J0""#);
    assert_eq!(StoredClaim::decode(&confirmed)?.lease(), &lease);
    let pending = StoredClaim::pending(&lease, TTL).encode()?;
    let decoded = StoredClaim::decode(&pending)?;
    assert!(decoded.is_pending(SystemTime::now()));
    assert!(!decoded.is_pending(SystemTime::now() + TTL * 2));
    Ok(())
}

#[tokio::test]
async fn nats_exact_owner_clear_preserves_a_replacement_mapping() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let js = jetstream::new(server.client.clone());
    let old_broker = NatsKvNamedLeaseStore::new(js.clone()).await?;
    let new_broker = NatsKvNamedLeaseStore::new(js).await?;
    let tenant = tenant("tenant")?;
    let old = LeaseId::new("old");
    let new = LeaseId::new("new");
    old_broker.set(&tenant, "workspace", &old).await?;
    new_broker.set(&tenant, "workspace", &new).await?;
    assert!(!old_broker.clear_if_matches(&tenant, "workspace", &old).await?);
    assert_eq!(new_broker.get(&tenant, "workspace").await?, Some(new.clone()));
    assert!(new_broker.clear_if_matches(&tenant, "workspace", &new).await?);
    assert!(old_broker.get(&tenant, "workspace").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn names_that_fold_to_one_legacy_key_are_claimed_independently() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let nats = NatsKvNamedLeaseStore::new(jetstream::new(server.client.clone())).await?;
    let memory = InMemoryNamedLeaseStore::new();
    let stores: [&dyn NamedLeaseStore; 2] = [&nats, &memory];
    for store in stores {
        let pairs = [
            (tenant("acme")?, "x_y", LeaseId::new("a")),
            (tenant("acme_x")?, "y", LeaseId::new("b")),
            (tenant("acme")?, "x.y", LeaseId::new("c")),
            (tenant("acme:x")?, "y", LeaseId::new("d")),
        ];
        for (tenant, name, lease) in &pairs {
            assert_eq!(
                store.claim(tenant, name, lease, TTL).await?,
                NameClaim::Claimed,
                "{tenant:?} {name}"
            );
        }
        for (tenant, name, lease) in &pairs {
            assert_eq!(store.get(tenant, name).await?.as_ref(), Some(lease));
        }
    }
    Ok(())
}

#[tokio::test]
async fn nats_legacy_keyed_name_still_resolves_and_releases() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvNamedLeaseStore::new(jetstream::new(server.client.clone())).await?;
    let tenant = tenant("acme.x")?;
    let lease = LeaseId::new("old");
    let legacy = legacy_named_lease_key(&tenant, "y");
    assert_ne!(legacy, named_lease_key(&tenant, "y"));
    store
        .kv
        .put(legacy, StoredClaim::Confirmed(lease.clone()).encode()?)
        .await?;

    assert_eq!(store.get(&tenant, "y").await?, Some(lease.clone()));
    assert!(!store.clear_if_matches(&tenant, "y", &LeaseId::new("other")).await?);
    assert!(store.clear_if_matches(&tenant, "y", &lease).await?);
    assert!(store.get(&tenant, "y").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn nats_concurrent_claims_have_exactly_one_winner() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let js = jetstream::new(server.client.clone());
    let store = Arc::new(NatsKvNamedLeaseStore::new(js).await?);
    let tenant = tenant("tenant")?;

    let mut claimers = Vec::new();
    for n in 0..8 {
        let store = store.clone();
        let tenant = tenant.clone();
        claimers.push(tokio::spawn(async move {
            let lease = LeaseId::new(format!("lease-{n}"));
            let outcome = store
                .claim(&tenant, "workspace", &lease, TTL)
                .await
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((lease, outcome))
        }));
    }
    let mut winners = Vec::new();
    let mut losers = Vec::new();
    for claimer in claimers {
        let (lease, outcome) = claimer.await??;
        match outcome {
            NameClaim::Claimed => winners.push(lease),
            NameClaim::Held { lease: holder, .. } => losers.push(holder),
        }
    }
    assert_eq!(winners.len(), 1, "{winners:?}");
    assert!(losers.iter().all(|holder| *holder == winners[0]));
    // A retry by the winner is idempotent; a stale-holder swap is CAS-checked.
    assert_eq!(
        store.claim(&tenant, "workspace", &winners[0], TTL).await?,
        NameClaim::Claimed
    );
    let other = LeaseId::new("other");
    assert!(
        !store
            .replace_if_matches(&tenant, "workspace", &other, &other, TTL)
            .await?
    );
    assert!(
        store
            .replace_if_matches(&tenant, "workspace", &winners[0], &other, TTL)
            .await?
    );
    Ok(())
}

#[tokio::test]
async fn a_cleared_name_reads_back_as_free() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvNamedLeaseStore::new(jetstream::new(server.client.clone())).await?;
    let tenant = tenant("acme")?;

    store.set(&tenant, "workspace", &LeaseId::new("01J0")).await?;
    store.clear(&tenant, "workspace").await?;

    assert!(store.get(&tenant, "workspace").await?.is_none());
    Ok(())
}
