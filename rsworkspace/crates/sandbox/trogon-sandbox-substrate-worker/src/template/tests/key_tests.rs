use std::error::Error;

use super::{AGENT_IMAGE, IMAGE, key, key_with};
use crate::config::SandboxClass;
use crate::template::constants::TEMPLATE_KEY_HEX_LEN;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn the_key_is_stable_and_content_addressed() -> TestResult {
    assert_eq!(key()?, key()?);
    assert_eq!(key()?.as_str().len(), TEMPLATE_KEY_HEX_LEN);
    assert_eq!(key()?.template_name(), format!("tpl-{}", key()?.as_str()));
    Ok(())
}

#[test]
fn every_field_that_invalidates_the_golden_snapshot_changes_the_key() -> TestResult {
    let other_image = "registry.example/other@sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let other_agent = "registry.example/agent@sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    let base = key()?;
    let variants = [
        key_with(other_image, SandboxClass::Gvisor, AGENT_IMAGE, 8110, "small")?,
        key_with(IMAGE, SandboxClass::MicroVm, AGENT_IMAGE, 8110, "small")?,
        key_with(IMAGE, SandboxClass::Gvisor, other_agent, 8110, "small")?,
        key_with(IMAGE, SandboxClass::Gvisor, AGENT_IMAGE, 9000, "small")?,
        key_with(IMAGE, SandboxClass::Gvisor, AGENT_IMAGE, 8110, "medium")?,
    ];
    for variant in variants {
        assert_ne!(base.as_str(), variant.as_str());
    }
    Ok(())
}

#[test]
fn the_accessors_expose_the_workload_identity() -> TestResult {
    let key = key()?;
    assert_eq!(key.image_reference(), IMAGE);
    assert_eq!(key.size_class_label(), "small");
    assert_eq!(
        key.kv_key("tenant-a"),
        format!("tenant-a.{}.{}", SandboxClass::Gvisor.as_str(), key.as_str())
    );
    Ok(())
}
