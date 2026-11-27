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
