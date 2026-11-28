use std::collections::BTreeMap;

use super::*;
use crate::test_fixtures::TestResult;

fn digest() -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::new(format!("sha256:{}", "b".repeat(64)))?)
}

fn annotations(registry: &str, repository: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (ANNOTATION_SOURCE_REGISTRY.to_owned(), registry.to_owned()),
        (ANNOTATION_SOURCE_REPOSITORY.to_owned(), repository.to_owned()),
    ])
}

#[test]
fn a_pod_that_recorded_its_source_gives_it_back() -> TestResult {
    let source = recovered_source(Some(&annotations("registry.example", "warm/base")), &digest()?);
    assert_eq!(
        source,
        Some(OciArtifactRef {
            registry: "registry.example".to_owned(),
            repository: "warm/base".to_owned(),
            digest: digest()?,
            artifact_type: ArtifactType::SandboxRootfsV1,
        })
    );
    Ok(())
}

#[test]
fn a_pod_without_a_usable_source_gives_none() -> TestResult {
    assert_eq!(recovered_source(None, &digest()?), None);
    assert_eq!(recovered_source(Some(&BTreeMap::new()), &digest()?), None);
    assert_eq!(recovered_source(Some(&annotations("", "warm/base")), &digest()?), None);
    assert_eq!(
        recovered_source(Some(&annotations("registry.example", "")), &digest()?),
        None
    );
    Ok(())
}

fn fill(fill: char) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::new(format!("sha256:{}", fill.to_string().repeat(64)))?)
}

fn config(per_digest: u32, total: u32, per_pass: u32, warming: u32) -> WarmPoolConfig {
    WarmPoolConfig {
        max_pool_size_per_digest: per_digest,
        max_total_warm: total,
        max_idle_seconds: 600,
        refill_per_pass: per_pass,
        max_warming: warming,
    }
}

#[test]
fn refill_is_paced_and_capped() -> TestResult {
    let (a, b) = (fill('a')?, fill('b')?);
    let digests = [a.clone(), b.clone()];
    let none = HashMap::new();

    // The per-pass pace stops creation across digests, in the order given.
    let plan = plan_refill(&config(5, 100, 3, 100), &digests, &none, &none);
    assert_eq!(plan, vec![(a.clone(), 3)]);

    // The per-digest bound counts ready and warming pods alike.
    let ready = HashMap::from([(a.clone(), 2)]);
    let warming = HashMap::from([(a.clone(), 1)]);
    let plan = plan_refill(&config(4, 100, 10, 100), &digests, &ready, &warming);
    assert_eq!(plan, vec![(a.clone(), 1), (b.clone(), 4)]);

    // The total bound counts every pod, whatever its digest.
    let ready = HashMap::from([(a.clone(), 3)]);
    let plan = plan_refill(&config(5, 5, 10, 100), &digests, &ready, &none);
    assert_eq!(plan, vec![(a.clone(), 2)]);

    // The warming bound counts only pods still starting.
    let warming = HashMap::from([(b.clone(), 3)]);
    let plan = plan_refill(&config(5, 100, 10, 4), &digests, &none, &warming);
    assert_eq!(plan, vec![(a.clone(), 1)]);

    // A full pool plans nothing.
    let ready = HashMap::from([(a.clone(), 5), (b.clone(), 5)]);
    assert!(plan_refill(&config(5, 100, 10, 100), &digests, &ready, &none).is_empty());
    Ok(())
}

#[test]
fn a_warm_restore_secret_name_is_a_valid_object_name() {
    let first = new_warm_restore_secret_name();
    let second = new_warm_restore_secret_name();
    assert_ne!(first, second);
    for name in [&first, &second] {
        assert!(name.starts_with(WARM_RESTORE_SECRET_PREFIX));
        assert!(name.len() <= 63);
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{name}"
        );
    }
}

#[test]
fn sources_for_evicted_digests_are_forgotten() -> TestResult {
    let (kept, evicted) = (fill('a')?, fill('c')?);
    let source = |digest: &Digest, repository: &str| OciArtifactRef {
        registry: "recorded.example".to_owned(),
        repository: repository.to_owned(),
        digest: digest.clone(),
        artifact_type: ArtifactType::SandboxRootfsV1,
    };
    let mut sources = HashMap::from([
        (kept.clone(), source(&kept, "kept")),
        (evicted.clone(), source(&evicted, "evicted")),
    ]);
    let picked = pick_source(
        &mut sources,
        std::slice::from_ref(&kept),
        &kept,
        "fallback.example",
        "fallback",
    );
    assert_eq!(picked, source(&kept, "kept"));
    assert!(!sources.contains_key(&evicted));

    // An evicted digest resolves to the fallback instead of its stale source.
    let mut sources = HashMap::from([(evicted.clone(), source(&evicted, "evicted"))]);
    let picked = pick_source(&mut sources, &[], &evicted, "fallback.example", "fallback");
    assert_eq!(picked.registry, "fallback.example");
    assert_eq!(picked.repository, "fallback");
    assert!(sources.is_empty());
    Ok(())
}
