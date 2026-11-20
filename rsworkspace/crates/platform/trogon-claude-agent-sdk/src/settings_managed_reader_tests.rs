#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::Path;

use super::{ManagedReader, ParsedFiles};
use crate::{
    SETTINGS_DEPTH_BOUND,
    constants::MANAGED_SETTINGS_FILE,
    settings_dialect::SettingsDialect,
    settings_resolve_error::ResolveSettingsError,
    settings_resolve_tier::{LoadState, TierRead},
    settings_resolve_values::SettingsMap,
};

fn nested(depth: usize) -> Vec<u8> {
    format!("{}null{}", "{\"x\":".repeat(depth - 1), "}".repeat(depth - 1)).into_bytes()
}

#[test]
fn the_typescript_dialect_reads_a_blank_managed_document_as_empty_settings() {
    let parsed = ParsedFiles::default();
    let reader = ManagedReader::new(SettingsDialect::TypeScript, &parsed);
    let read = reader
        .read(b" \n\t\r\n", Path::new(MANAGED_SETTINGS_FILE))
        .expect("reads");
    assert_eq!(
        (read.load_state, read.settings, read.has_policy_content, read.unreadable),
        (LoadState::Loaded, Some(SettingsMap::new()), false, false)
    );
}

#[test]
fn the_typescript_dialect_fails_a_managed_document_nested_at_the_bound() {
    let parsed = ParsedFiles::default();
    let reader = ManagedReader::new(SettingsDialect::TypeScript, &parsed);
    let path = Path::new(MANAGED_SETTINGS_FILE);
    assert!(reader.read(&nested(SETTINGS_DEPTH_BOUND - 1), path).is_ok());
    assert!(matches!(
        reader.read(&nested(SETTINGS_DEPTH_BOUND), path),
        Err(ResolveSettingsError::MaximumCallStack)
    ));
}

#[test]
fn the_python_dialect_reads_a_managed_document_as_utf8_with_replacements() {
    let parsed = ParsedFiles::default();
    let reader = ManagedReader::new(SettingsDialect::Python, &parsed);
    let path = Path::new(MANAGED_SETTINGS_FILE);
    let blank = reader.read(b"  ", path).expect("reads");
    assert_eq!(blank.settings, Some(SettingsMap::new()));
    let invalid = reader.read(b"[1]", path).expect("reads");
    assert_eq!(invalid.load_state, LoadState::DidNotLoad);
    let object = reader.read(b"{}", path).expect("reads");
    assert_eq!(object.load_state, LoadState::Loaded);
}

#[test]
fn the_typescript_dialect_parses_a_file_once_and_does_not_cache_a_failed_read() {
    let parsed = ParsedFiles::default();
    let reader = ManagedReader::new(SettingsDialect::TypeScript, &parsed);
    let path = Path::new(MANAGED_SETTINGS_FILE);
    let first = reader
        .read_file(path, |_| Ok((TierRead::absent(), true)))
        .expect("reads");
    let second = reader
        .read_file(path, |_| panic!("the parse cache answers"))
        .expect("reads");
    assert_eq!((first.1, second.1), (true, true));

    let other = Path::new("/etc/claude-code/managed-settings.d/a.json");
    let failed = reader.read_file(other, |_| Err(ResolveSettingsError::MaximumCallStack));
    assert!(failed.is_err());
    let retried = reader
        .read_file(other, |_| Ok((TierRead::absent(), false)))
        .expect("reads");
    assert!(!retried.1);
}

#[test]
fn the_python_dialect_does_not_cache() {
    let parsed = ParsedFiles::default();
    let reader = ManagedReader::new(SettingsDialect::Python, &parsed);
    let path = Path::new(MANAGED_SETTINGS_FILE);
    let first = reader
        .read_file(path, |_| Ok((TierRead::absent(), true)))
        .expect("reads");
    let second = reader
        .read_file(path, |_| Ok((TierRead::absent(), false)))
        .expect("reads");
    assert_eq!((first.1, second.1), (true, false));
}
