use std::error::Error;

use super::*;

fn digest(fill: char) -> Result<Digest, Box<dyn Error>> {
    Ok(Digest::new(format!("sha256:{}", fill.to_string().repeat(64)))?)
}

#[test]
fn evicts_oldest_when_over_capacity() -> Result<(), Box<dyn Error>> {
    let cache = DigestCache::new(2);
    let (d1, d2, d3) = (digest('1')?, digest('2')?, digest('3')?);
    cache.insert(d1.clone());
    cache.insert(d2.clone());
    cache.insert(d3.clone());
    let listed = cache.list();
    assert_eq!(listed.len(), 2);
    assert!(!listed.contains(&d1));
    assert!(listed.contains(&d2));
    assert!(listed.contains(&d3));
    Ok(())
}

#[test]
fn reinserting_moves_a_digest_to_the_newest_slot() -> Result<(), Box<dyn Error>> {
    let cache = DigestCache::new(2);
    let (d1, d2) = (digest('1')?, digest('2')?);
    cache.insert(d1.clone());
    cache.insert(d2.clone());
    cache.insert(d1.clone());
    assert_eq!(cache.list(), vec![d2.clone(), d1.clone()]);
    assert!(cache.contains(&d1));
    assert!(!cache.contains(&digest('3')?));
    Ok(())
}

#[test]
fn a_zero_capacity_still_keeps_one_digest() -> Result<(), Box<dyn Error>> {
    let cache = DigestCache::new(0);
    cache.insert(digest('1')?);
    cache.insert(digest('2')?);
    assert_eq!(cache.list(), vec![digest('2')?]);
    Ok(())
}

#[test]
fn a_poisoned_lock_is_still_usable() -> Result<(), Box<dyn Error>> {
    let cache = std::sync::Arc::new(DigestCache::new(2));
    let poisoner = std::sync::Arc::clone(&cache);
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.inner.lock();
        std::panic::resume_unwind(Box::new("poison"));
    })
    .join();
    cache.insert(digest('1')?);
    assert!(cache.contains(&digest('1')?));
    Ok(())
}
