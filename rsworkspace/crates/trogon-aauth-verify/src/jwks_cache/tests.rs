use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

use super::*;
use crate::jwks::StaticJwks;

#[derive(Clone, Default)]
struct MockTime {
    now: Arc<AtomicI64>,
}

impl MockTime {
    fn at(secs: i64) -> Self {
        Self {
            now: Arc::new(AtomicI64::new(secs)),
        }
    }
    fn advance(&self, secs: i64) {
        self.now.fetch_add(secs, Ordering::SeqCst);
    }
}

impl TimeSource for MockTime {
    fn now(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }
}

struct CountingResolver {
    inner: StaticJwks,
    calls: AtomicUsize,
    fail_with: Mutex<Option<JwksError>>,
}

impl CountingResolver {
    fn new(inner: StaticJwks) -> Self {
        Self {
            inner,
            calls: AtomicUsize::new(0),
            fail_with: Mutex::new(None),
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn fail_next(&self, err: JwksError) {
        *self.fail_with.lock().unwrap() = Some(err);
    }
}

#[async_trait]
impl JwksResolver for CountingResolver {
    async fn resolve(&self, iss: &str) -> Result<JwkSet, JwksError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(err) = self.fail_with.lock().unwrap().take() {
            return Err(err);
        }
        self.inner.resolve(iss).await
    }
}

fn empty_set() -> JwkSet {
    JwkSet { keys: vec![] }
}

#[tokio::test]
async fn hits_inner_once_within_ttl() {
    let inner = CountingResolver::new(StaticJwks::new().with("iss.example", empty_set()));
    let cache = CachedJwksResolver::new(inner, MockTime::at(0)).with_ttl_secs(60);

    cache.resolve("iss.example").await.expect("first hit");
    cache.resolve("iss.example").await.expect("cached hit");
    cache.resolve("iss.example").await.expect("cached hit");

    assert_eq!(cache.inner.calls(), 1);
}

#[tokio::test]
async fn re_fetches_after_ttl_expires() {
    let inner = CountingResolver::new(StaticJwks::new().with("iss.example", empty_set()));
    let time = MockTime::at(0);
    let cache = CachedJwksResolver::new(inner, time.clone()).with_ttl_secs(60);

    cache.resolve("iss.example").await.expect("first hit");
    time.advance(61);
    cache.resolve("iss.example").await.expect("re-fetched");

    assert_eq!(cache.inner.calls(), 2);
}

#[tokio::test]
async fn negative_cache_dampens_failure_storms() {
    let inner = CountingResolver::new(StaticJwks::new().with("iss.example", empty_set()));
    inner.fail_next(JwksError::Transport("boom".into()));
    let cache = CachedJwksResolver::new(inner, MockTime::at(0)).with_negative_ttl_secs(30);

    let first = cache.resolve("iss.example").await;
    assert!(matches!(first, Err(JwksError::Transport(_))));
    let second = cache.resolve("iss.example").await;
    assert!(matches!(second, Err(JwksError::Transport(_))));

    assert_eq!(cache.inner.calls(), 1, "negative cache held the second call");
}

#[tokio::test]
async fn cached_miss_preserves_original_variant() {
    // Regression: previously the cached negative entry stored a `String`
    // and reconstructed every replay as `JwksError::Transport(...)`,
    // even when the first miss was `UnknownIssuer` or `Malformed`.
    let inner = CountingResolver::new(StaticJwks::new());
    inner.fail_next(JwksError::UnknownIssuer("iss.example".into()));
    let cache = CachedJwksResolver::new(inner, MockTime::at(0)).with_negative_ttl_secs(60);

    let first = cache.resolve("iss.example").await;
    assert!(
        matches!(&first, Err(JwksError::UnknownIssuer(_))),
        "first miss returns original variant, got {first:?}"
    );
    let replayed = cache.resolve("iss.example").await;
    assert!(
        matches!(&replayed, Err(JwksError::UnknownIssuer(_))),
        "cached replay must preserve UnknownIssuer, got {replayed:?}"
    );
}

#[tokio::test]
async fn invalidate_mid_fetch_triggers_refetch_and_drops_stale_result() {
    // Regression for the high-severity finding: when generation moves
    // between snapshot and write, the cache must DROP the in-flight
    // outcome and re-fetch — otherwise a pre-rotation resolve started
    // before invalidate() can still hand stale keys back to the verifier.
    use tokio::sync::Notify;

    struct GenBumper {
        calls: AtomicUsize,
        bump_until: usize,
        sentinel: std::sync::Mutex<Option<Arc<Notify>>>,
    }
    #[async_trait]
    impl JwksResolver for GenBumper {
        async fn resolve(&self, _iss: &str) -> Result<JwkSet, JwksError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.bump_until
                && let Some(notify) = self.sentinel.lock().unwrap().as_ref()
            {
                notify.notify_one();
            }
            Ok(empty_set())
        }
    }

    let bumper = Arc::new(GenBumper {
        calls: AtomicUsize::new(0),
        bump_until: 1,
        sentinel: std::sync::Mutex::new(None),
    });
    let cache = Arc::new(CachedJwksResolver::new(bumper.clone(), MockTime::at(0)));

    let notify = Arc::new(Notify::new());
    *bumper.sentinel.lock().unwrap() = Some(notify.clone());
    let invalidator = {
        let cache = cache.clone();
        tokio::spawn(async move {
            notify.notified().await;
            cache.invalidate("iss").await;
        })
    };

    let _ = cache.resolve("iss").await;
    invalidator.await.unwrap();

    let calls_before = bumper.calls.load(Ordering::SeqCst);
    let _ = cache.resolve("iss").await;
    let calls_after = bumper.calls.load(Ordering::SeqCst);
    assert!(
        calls_after > calls_before,
        "follow-up resolve must re-fetch when cache was invalidated; \
         calls_before={calls_before}, calls_after={calls_after}"
    );
}

#[tokio::test]
async fn slow_path_returns_peer_cache_entry_when_one_appears_mid_flight() {
    // Regression for the medium-severity race: when two callers fetched
    // the same iss concurrently and a peer landed a fresh Hit first, the
    // loser used to return its OWN outcome (which could be a transient
    // error) rather than honor the cached Hit. Verify the slow path now
    // returns the peer's cached entry instead.
    use tokio::sync::oneshot;

    struct GatedInner {
        gate: std::sync::Mutex<Option<oneshot::Receiver<()>>>,
    }
    #[async_trait]
    impl JwksResolver for GatedInner {
        async fn resolve(&self, _iss: &str) -> Result<JwkSet, JwksError> {
            let rx = self.gate.lock().unwrap().take();
            if let Some(rx) = rx {
                let _ = rx.await;
            }
            Err(JwksError::Transport("transient inner failure".into()))
        }
    }

    let (tx, rx) = oneshot::channel();
    let inner = GatedInner {
        gate: std::sync::Mutex::new(Some(rx)),
    };
    let cache = Arc::new(CachedJwksResolver::new(inner, MockTime::at(0)));

    let loser = {
        let cache = cache.clone();
        tokio::spawn(async move { cache.resolve("iss.example").await })
    };

    tokio::task::yield_now().await;
    tokio::task::yield_now().await;

    cache.entries.write().await.insert(
        "iss.example".to_string(),
        CachedEntry::Hit {
            keys: empty_set(),
            expires_at: 600,
        },
    );

    let _ = tx.send(());
    let got = loser.await.unwrap();
    assert!(
        got.is_ok(),
        "loser must surface the peer's fresh cached Hit, not its own Err: {got:?}"
    );
}

#[tokio::test]
async fn invalidate_blocks_inflight_refill() {
    // Regression: previously a slow `inner.resolve` would write its result
    // back unconditionally after the await, undoing an `invalidate()` that
    // raced with it. The generation counter must block that write.
    let inner = CountingResolver::new(StaticJwks::new().with("iss.example", empty_set()));
    let cache = CachedJwksResolver::new(inner, MockTime::at(0));

    cache.resolve("iss.example").await.expect("first hit");
    cache.invalidate("iss.example").await;
    cache.resolve("iss.example").await.expect("re-fetched");
    assert_eq!(cache.inner.calls(), 2);
}

#[tokio::test]
async fn max_entries_caps_distinct_issuers() {
    // Regression: `iss` is attacker-controlled until signature checks,
    // so an attacker can pump unique issuers and grow the map unbounded.
    let inner = CountingResolver::new(StaticJwks::new());
    let cache = CachedJwksResolver::new(inner, MockTime::at(0))
        .with_max_entries(2)
        .with_negative_ttl_secs(60);
    let _ = cache.resolve("iss-a").await;
    let _ = cache.resolve("iss-b").await;
    let _ = cache.resolve("iss-c").await;
    let _ = cache.resolve("iss-d").await;
    let guard = cache.entries.read().await;
    assert!(guard.len() <= 2, "map grew past max_entries: {}", guard.len());
}

#[tokio::test]
async fn entry_expiry_anchors_to_clock_after_inner_resolve_not_before() {
    // Regression: `now` was captured once at the top of resolve() and
    // reused after the slow inner.resolve await, so on a slow fetch the
    // entry's expires_at = old_now + ttl outlived the configured TTL by
    // the duration of the fetch. Verify expires_at uses the post-await
    // clock value.
    use tokio::sync::oneshot;

    struct GatedOk {
        gate: std::sync::Mutex<Option<oneshot::Receiver<()>>>,
    }
    #[async_trait]
    impl JwksResolver for GatedOk {
        async fn resolve(&self, _iss: &str) -> Result<JwkSet, JwksError> {
            let rx_opt = self.gate.lock().unwrap().take();
            if let Some(rx) = rx_opt {
                let _ = rx.await;
            }
            Ok(empty_set())
        }
    }

    let (tx, rx) = oneshot::channel();
    let inner = GatedOk {
        gate: std::sync::Mutex::new(Some(rx)),
    };
    let time = MockTime::at(100);
    let cache = Arc::new(CachedJwksResolver::new(inner, time.clone()).with_ttl_secs(60));

    let fetcher = {
        let cache = cache.clone();
        tokio::spawn(async move { cache.resolve("iss").await })
    };

    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    time.advance(500);

    let _ = tx.send(());
    fetcher.await.unwrap().expect("fetch succeeds");

    let entry = cache.entries.read().await;
    match entry.get("iss") {
        Some(CachedEntry::Hit { expires_at, .. }) => {
            assert!(
                *expires_at >= 660,
                "expires_at must anchor to post-await clock (>=660), got {expires_at}"
            );
        }
        _ => panic!("expected a Hit entry for 'iss'"),
    }
}

#[tokio::test]
async fn invalidate_all_drops_every_entry() {
    let inner = CountingResolver::new(StaticJwks::new().with("iss.a", empty_set()).with("iss.b", empty_set()));
    let cache = CachedJwksResolver::new(inner, MockTime::at(0));
    cache.resolve("iss.a").await.expect("a");
    cache.resolve("iss.b").await.expect("b");
    cache.invalidate_all().await;
    cache.resolve("iss.a").await.expect("a refetched");
    cache.resolve("iss.b").await.expect("b refetched");
    assert_eq!(cache.inner.calls(), 4);
}

#[tokio::test]
async fn with_max_entries_zero_is_clamped_to_one() {
    let inner = CountingResolver::new(StaticJwks::new().with("iss", empty_set()));
    let cache = CachedJwksResolver::new(inner, MockTime::at(0)).with_max_entries(0);
    cache.resolve("iss").await.expect("hit");
    // max_entries was 0; constructor clamps to 1 so the entry still lives.
    assert!(!cache.entries.read().await.is_empty());
}

#[tokio::test]
async fn invalidate_clears_entry() {
    let inner = CountingResolver::new(StaticJwks::new().with("iss.example", empty_set()));
    let cache = CachedJwksResolver::new(inner, MockTime::at(0));

    cache.resolve("iss.example").await.expect("first hit");
    cache.invalidate("iss.example").await;
    cache.resolve("iss.example").await.expect("re-fetched");

    assert_eq!(cache.inner.calls(), 2);
}
