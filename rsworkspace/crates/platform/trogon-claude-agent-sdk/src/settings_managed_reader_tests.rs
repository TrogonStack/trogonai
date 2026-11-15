use std::cell::Cell;

use super::{ManagedReader, ParsedFiles};
use crate::{
    node_merge_warmth::MergeWarmth, node_stack::NodeStack, settings_dialect::SettingsDialect,
    settings_resolve_tier::LoadState, settings_resolve_values::SettingsMap,
};

#[test]
fn the_typescript_dialect_reads_a_blank_managed_document_as_empty_settings() {
    let parsed = ParsedFiles::default();
    let warmth = Cell::new(MergeWarmth::default());
    let reader = ManagedReader::new(
        SettingsDialect::TypeScript,
        NodeStack::NODE_24_DARWIN_ARM64,
        &parsed,
        &warmth,
    );
    let read = reader.read(b" \n\t\r\n");
    assert_eq!(
        (read.load_state, read.settings, read.has_policy_content, read.unreadable),
        (LoadState::Loaded, Some(SettingsMap::new()), false, false)
    );
}
