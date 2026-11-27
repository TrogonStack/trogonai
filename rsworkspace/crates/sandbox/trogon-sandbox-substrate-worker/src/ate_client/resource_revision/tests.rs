use super::*;
use crate::adapter::tests_support::TestResult;

fn metadata() -> proto::ResourceMetadata {
    proto::ResourceMetadata {
        atespace: "tenant-a".to_string(),
        name: "actor-1".to_string(),
        uid: "uid-1".to_string(),
        version: 7,
        ..Default::default()
    }
}

fn invalid(result: AteResult<ResourceRevision>) -> TestResult {
    match result {
        Err(AteError::InvalidArgument { .. }) => Ok(()),
        other => Err(format!("expected InvalidArgument, got {other:?}").into()),
    }
}

#[test]
fn a_revision_carries_the_observed_address_uid_and_version() -> TestResult {
    let revision = ResourceRevision::from_metadata(Some(&metadata()))?;

    assert_eq!(revision.resource().atespace, "tenant-a");
    assert_eq!(revision.resource().name, "actor-1");
    let options = revision.options();
    assert_eq!(options.uid, "uid-1");
    assert_eq!(options.version, 7);
    Ok(())
}

#[test]
fn missing_metadata_is_refused() -> TestResult {
    invalid(ResourceRevision::from_metadata(None))
}

#[test]
fn every_part_of_the_observation_is_required() -> TestResult {
    let mut no_atespace = metadata();
    no_atespace.atespace.clear();
    let mut no_name = metadata();
    no_name.name.clear();
    let mut no_uid = metadata();
    no_uid.uid.clear();
    let mut no_version = metadata();
    no_version.version = 0;
    let mut negative_version = metadata();
    negative_version.version = -1;

    for incomplete in [no_atespace, no_name, no_uid, no_version, negative_version] {
        invalid(ResourceRevision::from_metadata(Some(&incomplete)))?;
    }
    Ok(())
}
