use super::*;

/// A cache that compiled `pattern` and then `fillers` other patterns.
fn after_fillers(cache: &mut ReCache, pattern: &str, fillers: std::ops::Range<usize>) {
    cache.compiled(pattern);
    for filler in fillers {
        cache.compiled(format!("x{filler}").as_str());
    }
}

#[test]
fn a_pattern_stays_until_both_caches_drop_it() {
    let mut cache = ReCache::default();
    assert!(!cache.contains("p"));
    after_fillers(&mut cache, "p", 0..255);
    assert!(cache.contains("p"));
    assert!(cache.recent.contains(&Key::from("p")));
    cache.compiled("x255");
    assert!(!cache.recent.contains(&Key::from("p")));
    assert!(cache.contains("p"));
    for filler in 256..511 {
        cache.compiled(format!("x{filler}").as_str());
    }
    assert!(cache.contains("p"));
    cache.compiled("x511");
    assert!(!cache.contains("p"));
    assert_eq!(cache.used.len(), MAX_CACHE);
    assert_eq!(cache.recent.len(), MAX_CACHE2);
}

#[test]
fn a_pattern_found_only_in_the_lru_cache_is_used_again_and_rejoins_the_fifo_cache() {
    let mut cache = ReCache::default();
    after_fillers(&mut cache, "p", 0..300);
    cache.found("p");
    assert!(cache.recent.contains(&Key::from("p")));
    for filler in 300..811 {
        cache.compiled(format!("x{filler}").as_str());
    }
    assert!(cache.contains("p"));
    cache.compiled("x811");
    assert!(!cache.contains("p"));
}

#[test]
fn a_pattern_found_in_the_fifo_cache_is_not_used_again() {
    let mut cache = ReCache::default();
    after_fillers(&mut cache, "p", 0..200);
    cache.found("p");
    for filler in 200..511 {
        cache.compiled(format!("x{filler}").as_str());
    }
    assert!(cache.contains("p"));
    cache.compiled("x511");
    assert!(!cache.contains("p"));
}

#[test]
fn a_pattern_in_neither_cache_is_found_as_nothing() {
    let mut cache = ReCache::default();
    cache.found("p");
    assert!(!cache.contains("p"));
    assert!(cache.by_use.is_empty());
}

/// The keys of `_cache` and of `_cache2`, oldest first, each a pattern or `None` for a key `jsonschema` never looks
/// up.
fn keys(cache: &ReCache) -> (Vec<Option<String>>, Vec<Option<String>>) {
    let pattern = |key: &Key| match key {
        Key::Pattern(pattern) => Some(pattern.to_str_lossy().into_owned()),
        Key::Other(_) => None,
    };
    (
        cache.by_use.values().map(pattern).collect(),
        cache.by_insertion.iter().map(pattern).collect(),
    )
}

/// `process` of `python_re_cache.json` is `re._cache` as the pinned Python SDK's process holds it at its first tool
/// call, recorded from the real package.
#[test]
fn a_seeded_cache_holds_what_the_python_process_compiled_before_its_first_call() -> Result<(), String> {
    // Some cases nest their schemas past the default recursion limit of the parser.
    let mut deserializer =
        serde_json::Deserializer::from_str(include_str!("../tests/fixtures/sdk_mcp/python_re_cache.json"));
    deserializer.disable_recursion_limit();
    let fixture: serde_json::Value =
        serde::Deserialize::deserialize(&mut deserializer).map_err(|error| error.to_string())?;
    let process: Vec<Option<String>> = fixture
        .get("process")
        .and_then(serde_json::Value::as_array)
        .ok_or("the fixture has no process")?
        .iter()
        .map(|key| key.as_str().map(str::to_owned))
        .collect();
    let cache = ReCache::seeded();
    assert_eq!(keys(&cache), (process.clone(), process));
    assert!(cache.contains("^\\d+$"));
    assert!(!cache.contains("^\\d+%"));
    Ok(())
}

/// A key `jsonschema` never looks up still takes room: `^\d+$` is key 88 of the 150, counting from 0, so 450 new
/// patterns leave it in `_cache` and one more drops it, as its 61 newer keys and 451 new patterns make 512.
#[test]
fn keys_jsonschema_never_looks_up_still_fill_the_caches() {
    let mut cache = ReCache::seeded();
    for filler in 0..450 {
        cache.compiled(format!("[{filler}]x").as_str());
    }
    assert!(cache.contains("^\\d+$"));
    assert!(!cache.recent.contains(&Key::from("^\\d+$")));
    cache.compiled("[450]x");
    assert!(!cache.contains("^\\d+$"));
    assert_eq!(cache.used.len(), MAX_CACHE);
}

#[test]
fn the_process_cache_is_one_cache() {
    assert!(Arc::ptr_eq(&process_cache(), &process_cache()));
}
