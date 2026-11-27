use super::*;
use crate::support_tests::TestResult;

fn digest(label: &str) -> Result<Digest, trogon_sandbox_contract::DigestError> {
    Digest::new(format!("sha256:{label:0>64}"))
}

#[test]
fn touch_moves_digest_to_front_and_evicts_oldest() -> TestResult {
    let cache = DigestCache::new(2);
    let d1 = digest("aaa")?;
    let d2 = digest("bbb")?;
    let d3 = digest("ccc")?;
    cache.touch(d1.clone());
    cache.touch(d2.clone());
    cache.touch(d3.clone());
    assert_eq!(cache.digests(), vec![d3, d2]);
    assert!(!cache.contains(&d1));
    Ok(())
}

#[test]
fn touching_a_known_digest_refreshes_it_without_growing_the_cache() -> TestResult {
    let cache = DigestCache::new(0);
    let d1 = digest("aaa")?;
    cache.touch(d1.clone());
    cache.touch(d1.clone());
    assert_eq!(cache.digests(), vec![d1.clone()]);
    assert!(cache.contains(&d1));
    Ok(())
}

#[test]
fn a_poisoned_lock_still_serves_the_cache() -> TestResult {
    let cache = std::sync::Arc::new(DigestCache::new(2));
    let poisoner = std::sync::Arc::clone(&cache);
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.order.lock();
        std::panic::resume_unwind(Box::new("poison"));
    })
    .join();
    cache.touch(digest("aaa")?);
    assert_eq!(cache.digests().len(), 1);
    Ok(())
}
