use super::{DiscoveryMetadata, DiscoveryMetadataError};
use crate::discovery_metadata_input::DiscoveryMetadataInput;

#[test]
fn carries_the_entries_it_was_given() {
    let metadata = DiscoveryMetadata::from_input(&DiscoveryMetadataInput::new([("always-faults", "true")]))
        .expect("a named entry is valid");

    assert_eq!(
        metadata.entries().get("always-faults").map(String::as_str),
        Some("true")
    );
}

#[test]
fn is_empty_without_entries() {
    assert!(DiscoveryMetadata::default().is_empty());
}

/// A nameless key is not something a discovery consumer can ask for, so it is
/// rejected rather than published as `"": ...`.
#[test]
fn rejects_a_nameless_key() {
    assert_eq!(
        DiscoveryMetadata::from_input(&DiscoveryMetadataInput::new([("", "true")])),
        Err(DiscoveryMetadataError::EmptyKey)
    );
}
