use std::time::{Duration, UNIX_EPOCH};

use super::*;
use crate::source::plaid::constants::{KEY_FETCH_THROTTLE_INTERVAL, KEY_REFRESH_INTERVAL};
use crate::source::plaid::test_support::{FakeFailure, FakeKeyFetcher, TestKeyPair};

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

#[tokio::test]
async fn fetches_once_per_key_id() {
    let pair = TestKeyPair::generate("kid-1");
    let fetcher = FakeKeyFetcher::default().with_key("kid-1", pair.verification_key(None));
    let cache = PlaidKeyCache::new(fetcher.clone());

    for _ in 0..3 {
        let key = cache.resolve(&pair.key_id(), at(100)).await.unwrap();
        assert_eq!(key.key_id().as_str(), "kid-1");
    }

    assert_eq!(fetcher.calls(), vec!["kid-1"]);
}

#[tokio::test]
async fn fetches_again_for_a_new_key_id() {
    let first = TestKeyPair::generate("kid-1");
    let second = TestKeyPair::generate("kid-2");
    let fetcher = FakeKeyFetcher::default()
        .with_key("kid-1", first.verification_key(None))
        .with_key("kid-2", second.verification_key(None));
    let cache = PlaidKeyCache::new(fetcher.clone());

    let later = 100 + KEY_FETCH_THROTTLE_INTERVAL.as_secs();

    cache.resolve(&first.key_id(), at(100)).await.unwrap();
    cache.resolve(&second.key_id(), at(later)).await.unwrap();
    cache.resolve(&first.key_id(), at(later)).await.unwrap();

    assert_eq!(fetcher.calls(), vec!["kid-1", "kid-2"]);
}

#[tokio::test]
async fn throttles_a_second_unknown_key_id_inside_the_interval() {
    let first = TestKeyPair::generate("kid-1");
    let second = TestKeyPair::generate("kid-2");
    let fetcher = FakeKeyFetcher::default()
        .with_key("kid-1", first.verification_key(None))
        .with_key("kid-2", second.verification_key(None));
    let cache = PlaidKeyCache::new(fetcher.clone());

    cache.resolve(&first.key_id(), at(100)).await.unwrap();
    let err = cache
        .resolve(&second.key_id(), at(100 + KEY_FETCH_THROTTLE_INTERVAL.as_secs() - 1))
        .await
        .unwrap_err();

    assert!(matches!(err, PlaidKeyResolveError::Throttled(_)));
    assert_eq!(err.to_string(), "fetching verification key kid-2 was throttled");
    assert_eq!(fetcher.calls(), vec!["kid-1"]);
}

#[tokio::test]
async fn fetches_an_unknown_key_id_once_the_interval_elapses() {
    let first = TestKeyPair::generate("kid-1");
    let second = TestKeyPair::generate("kid-2");
    let fetcher = FakeKeyFetcher::default()
        .with_key("kid-1", first.verification_key(None))
        .with_key("kid-2", second.verification_key(None));
    let cache = PlaidKeyCache::new(fetcher.clone());
    let interval_secs = KEY_FETCH_THROTTLE_INTERVAL.as_secs();

    cache.resolve(&first.key_id(), at(100)).await.unwrap();
    let throttled = cache.resolve(&second.key_id(), at(100 + interval_secs - 1)).await;
    let key = cache.resolve(&second.key_id(), at(100 + interval_secs)).await.unwrap();

    assert!(matches!(throttled, Err(PlaidKeyResolveError::Throttled(_))));
    assert_eq!(key.key_id().as_str(), "kid-2");
    assert_eq!(fetcher.calls(), vec!["kid-1", "kid-2"]);
}

#[tokio::test]
async fn refreshing_a_stale_key_is_independent_of_the_unknown_key_throttle() {
    let first = TestKeyPair::generate("kid-1");
    let second = TestKeyPair::generate("kid-2");
    let third = TestKeyPair::generate("kid-3");
    let fetcher = FakeKeyFetcher::default()
        .with_key("kid-1", first.verification_key(None))
        .with_key("kid-2", second.verification_key(None))
        .with_key("kid-3", third.verification_key(None));
    let cache = PlaidKeyCache::new(fetcher.clone());
    let stale_at = 100 + KEY_REFRESH_INTERVAL.as_secs();

    cache.resolve(&first.key_id(), at(100)).await.unwrap();
    cache.resolve(&second.key_id(), at(stale_at)).await.unwrap();
    cache.resolve(&first.key_id(), at(stale_at + 1)).await.unwrap();
    cache
        .resolve(&third.key_id(), at(stale_at + KEY_FETCH_THROTTLE_INTERVAL.as_secs()))
        .await
        .unwrap();

    assert_eq!(fetcher.calls(), vec!["kid-1", "kid-2", "kid-1", "kid-3"]);
}

#[tokio::test]
async fn an_earlier_clock_reading_does_not_throttle_an_unknown_key_id() {
    let first = TestKeyPair::generate("kid-1");
    let second = TestKeyPair::generate("kid-2");
    let fetcher = FakeKeyFetcher::default()
        .with_key("kid-1", first.verification_key(None))
        .with_key("kid-2", second.verification_key(None));
    let cache = PlaidKeyCache::new(fetcher.clone());

    cache.resolve(&first.key_id(), at(100)).await.unwrap();
    cache.resolve(&second.key_id(), at(50)).await.unwrap();

    assert_eq!(fetcher.calls(), vec!["kid-1", "kid-2"]);
}

#[tokio::test]
async fn rejects_a_key_expired_at_fetch_time() {
    let pair = TestKeyPair::generate("kid-1");
    let fetcher = FakeKeyFetcher::default().with_key("kid-1", pair.verification_key(Some(50)));
    let cache = PlaidKeyCache::new(fetcher);

    let err = cache.resolve(&pair.key_id(), at(100)).await.unwrap_err();

    assert!(matches!(err, PlaidKeyResolveError::Expired(_)));
    assert_eq!(err.to_string(), "verification key kid-1 is expired");
}

#[tokio::test]
async fn rejects_a_cached_key_once_it_expires() {
    let pair = TestKeyPair::generate("kid-1");
    let fetcher = FakeKeyFetcher::default().with_key("kid-1", pair.verification_key(Some(200)));
    let cache = PlaidKeyCache::new(fetcher.clone());

    cache.resolve(&pair.key_id(), at(100)).await.unwrap();
    let err = cache.resolve(&pair.key_id(), at(200)).await.unwrap_err();

    assert!(matches!(err, PlaidKeyResolveError::Expired(_)));
    assert_eq!(fetcher.calls().len(), 1);
}

#[tokio::test]
async fn fetch_failures_are_not_cached() {
    let pair = TestKeyPair::generate("kid-1");
    let fetcher = FakeKeyFetcher::default().failing(FakeFailure::Unavailable);
    let cache = PlaidKeyCache::new(fetcher.clone());

    for now in [100, 100 + KEY_FETCH_THROTTLE_INTERVAL.as_secs()] {
        let err = cache.resolve(&pair.key_id(), at(now)).await.unwrap_err();
        assert!(matches!(err, PlaidKeyResolveError::Fetch(_)));
    }

    assert_eq!(fetcher.calls().len(), 2);
}

#[tokio::test]
async fn concurrent_resolves_share_one_fetch() {
    let pair = TestKeyPair::generate("kid-1");
    let fetcher = FakeKeyFetcher::default().with_key("kid-1", pair.verification_key(None));
    let cache = PlaidKeyCache::new(fetcher.clone());
    let key_id = pair.key_id();

    let (a, b, c) = tokio::join!(
        cache.resolve(&key_id, at(100)),
        cache.resolve(&key_id, at(100)),
        cache.resolve(&key_id, at(100)),
    );

    assert!(a.is_ok() && b.is_ok() && c.is_ok());
    assert_eq!(fetcher.calls().len(), 1);
}

#[tokio::test]
async fn refetches_a_cached_key_once_the_refresh_interval_elapses() {
    let pair = TestKeyPair::generate("kid-1");
    let fetcher = FakeKeyFetcher::default().with_key("kid-1", pair.verification_key(None));
    let cache = PlaidKeyCache::new(fetcher.clone());
    let refresh_secs = KEY_REFRESH_INTERVAL.as_secs();

    cache.resolve(&pair.key_id(), at(100)).await.unwrap();
    cache.resolve(&pair.key_id(), at(100 + refresh_secs - 1)).await.unwrap();
    assert_eq!(fetcher.calls().len(), 1);

    cache.resolve(&pair.key_id(), at(100 + refresh_secs)).await.unwrap();
    cache.resolve(&pair.key_id(), at(100 + refresh_secs + 1)).await.unwrap();
    assert_eq!(fetcher.calls().len(), 2);
}

#[tokio::test]
async fn rejects_a_key_that_a_refresh_reports_as_expired() {
    let pair = TestKeyPair::generate("kid-1");
    let fetcher = FakeKeyFetcher::default().with_key("kid-1", pair.verification_key(None));
    let cache = PlaidKeyCache::new(fetcher.clone());
    let refresh_secs = KEY_REFRESH_INTERVAL.as_secs();

    cache.resolve(&pair.key_id(), at(100)).await.unwrap();
    fetcher.insert_key("kid-1", pair.verification_key(Some(150)));

    let err = cache.resolve(&pair.key_id(), at(100 + refresh_secs)).await.unwrap_err();

    assert!(matches!(err, PlaidKeyResolveError::Expired(_)));
}

#[tokio::test]
async fn a_failed_refresh_does_not_fall_back_to_the_stale_key() {
    let pair = TestKeyPair::generate("kid-1");
    let fetcher = FakeKeyFetcher::default().with_key("kid-1", pair.verification_key(None));
    let cache = PlaidKeyCache::new(fetcher.clone());
    let refresh_secs = KEY_REFRESH_INTERVAL.as_secs();

    cache.resolve(&pair.key_id(), at(100)).await.unwrap();
    fetcher.fail_with(FakeFailure::Unavailable);

    let err = cache.resolve(&pair.key_id(), at(100 + refresh_secs)).await.unwrap_err();

    assert!(matches!(err, PlaidKeyResolveError::Fetch(_)));
    assert_eq!(fetcher.calls().len(), 2);
}

#[tokio::test]
async fn throttles_retrying_a_failed_refresh_inside_the_interval() {
    let pair = TestKeyPair::generate("kid-1");
    let fetcher = FakeKeyFetcher::default().with_key("kid-1", pair.verification_key(None));
    let cache = PlaidKeyCache::new(fetcher.clone());
    let stale_at = 100 + KEY_REFRESH_INTERVAL.as_secs();
    let interval_secs = KEY_FETCH_THROTTLE_INTERVAL.as_secs();

    cache.resolve(&pair.key_id(), at(100)).await.unwrap();
    fetcher.fail_with(FakeFailure::Unavailable);
    let failed = cache.resolve(&pair.key_id(), at(stale_at)).await;
    let throttled = cache.resolve(&pair.key_id(), at(stale_at + interval_secs - 1)).await;
    let retried = cache.resolve(&pair.key_id(), at(stale_at + interval_secs)).await;

    assert!(matches!(failed, Err(PlaidKeyResolveError::Fetch(_))));
    assert!(matches!(throttled, Err(PlaidKeyResolveError::Throttled(_))));
    assert!(matches!(retried, Err(PlaidKeyResolveError::Fetch(_))));
    assert_eq!(fetcher.calls().len(), 3);
}

#[tokio::test]
async fn a_failed_refresh_only_throttles_its_own_key_id() {
    let first = TestKeyPair::generate("kid-1");
    let second = TestKeyPair::generate("kid-2");
    let fetcher = FakeKeyFetcher::default()
        .with_key("kid-1", first.verification_key(None))
        .with_key("kid-2", second.verification_key(None));
    let cache = PlaidKeyCache::new(fetcher.clone());
    let interval_secs = KEY_FETCH_THROTTLE_INTERVAL.as_secs();
    let second_stale_at = 100 + interval_secs + KEY_REFRESH_INTERVAL.as_secs();

    cache.resolve(&first.key_id(), at(100)).await.unwrap();
    cache.resolve(&second.key_id(), at(100 + interval_secs)).await.unwrap();
    fetcher.fail_with(FakeFailure::Unavailable);
    let first_failed = cache
        .resolve(&first.key_id(), at(second_stale_at - interval_secs / 2))
        .await;
    let second_failed = cache.resolve(&second.key_id(), at(second_stale_at)).await;

    assert!(matches!(first_failed, Err(PlaidKeyResolveError::Fetch(_))));
    assert!(matches!(second_failed, Err(PlaidKeyResolveError::Fetch(_))));
    assert_eq!(fetcher.calls(), vec!["kid-1", "kid-2", "kid-1", "kid-2"]);
}

#[tokio::test]
async fn an_earlier_clock_reading_keeps_the_cached_key() {
    let pair = TestKeyPair::generate("kid-1");
    let fetcher = FakeKeyFetcher::default().with_key("kid-1", pair.verification_key(None));
    let cache = PlaidKeyCache::new(fetcher.clone());

    cache.resolve(&pair.key_id(), at(100)).await.unwrap();
    cache.resolve(&pair.key_id(), at(50)).await.unwrap();

    assert_eq!(fetcher.calls().len(), 1);
}
