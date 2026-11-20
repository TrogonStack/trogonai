//! How a resolve reads its managed settings documents in each dialect.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    node_text_file::read_node_text,
    session_prompt::js_trim,
    settings_dialect::SettingsDialect,
    settings_numbers::normalize_numbers,
    settings_parse::parse_json,
    settings_policy_parse::parse_policy_text,
    settings_read::read_policy_object,
    settings_resolve_error::ResolveSettingsError,
    settings_resolve_tier::{LoadState, TierRead},
    settings_resolve_values::SettingsMap,
};

/// The managed settings documents a resolve read so far, by path, and whether each read opened its file: the parse
/// cache of the file cache (`Fu` upstream), which every evaluation of the managed settings shares.
#[derive(Debug, Default)]
pub(crate) struct ParsedFiles(RefCell<HashMap<PathBuf, (TierRead, bool)>>);

/// The reader of a resolve's managed settings documents.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ManagedReader<'p> {
    decoder: Decoder,
    parsed: &'p ParsedFiles,
}

/// How a reader decodes and parses managed settings documents.
#[derive(Clone, Copy, Debug)]
enum Decoder {
    /// The Python dialect: the bytes as UTF-8 with replacement characters, parsed by `serde_json`.
    Lossy,
    /// The TypeScript dialect: the bytes as Node decodes them, parsed at any depth below the settings depth bound.
    Node,
}

impl<'p> ManagedReader<'p> {
    /// The reader of the managed settings, keeping what the TypeScript dialect parses in `parsed`.
    pub(crate) fn new(dialect: SettingsDialect, parsed: &'p ParsedFiles) -> Self {
        let decoder = match dialect {
            SettingsDialect::Python => Decoder::Lossy,
            SettingsDialect::TypeScript => Decoder::Node,
        };
        Self { decoder, parsed }
    }

    /// The managed settings document at `path`, from `read` the first time the TypeScript dialect reads it and from
    /// the parse cache after that. A document `read` fails on is not cached.
    pub(crate) fn read_file(
        self,
        path: &Path,
        read: impl FnOnce(Self) -> Result<(TierRead, bool), ResolveSettingsError>,
    ) -> Result<(TierRead, bool), ResolveSettingsError> {
        if !matches!(self.decoder, Decoder::Node) {
            return read(self);
        }
        let cached = self.parsed.0.borrow().get(path).cloned();
        if let Some(cached) = cached {
            return Ok(cached);
        }
        let fresh = read(self)?;
        self.parsed.0.borrow_mut().insert(path.to_path_buf(), fresh.clone());
        Ok(fresh)
    }

    /// A managed settings document from the bytes of its file at `path`, which fit the size limit, sanitized and read
    /// by the policy-tolerant schema parse in either dialect. The TypeScript dialect fails with
    /// [`ResolveSettingsError::MaximumCallStack`] for a document nested at the settings depth bound or deeper.
    pub(crate) fn read(self, bytes: &[u8], path: &Path) -> Result<TierRead, ResolveSettingsError> {
        let parsed = match self.decoder {
            Decoder::Lossy => {
                let text = String::from_utf8_lossy(bytes);
                if text.trim().is_empty() {
                    return Ok(loaded(SettingsMap::new()));
                }
                match parse_json(&text) {
                    Ok(Value::Object(document)) => Some(document),
                    _ => None,
                }
            }
            Decoder::Node => {
                let text = read_node_text(bytes);
                if js_trim(&text).is_empty() {
                    return Ok(loaded(SettingsMap::new()));
                }
                parse_policy_text(&text)?.map(|mut document| {
                    for entry in document.values_mut() {
                        normalize_numbers(entry, SettingsDialect::TypeScript);
                    }
                    document
                })
            }
        };
        Ok(parsed.map_or_else(not_loaded, |document| read_policy(document, path)))
    }
}

fn loaded(settings: SettingsMap) -> TierRead {
    TierRead {
        settings: Some(settings),
        load_state: LoadState::Loaded,
        ..TierRead::absent()
    }
}

/// `lE(..., true)` upstream for a document that parsed as an object.
fn read_policy(document: SettingsMap, path: &Path) -> TierRead {
    let read = read_policy_object(document, Some(&path.to_string_lossy()));
    read.map_or_else(not_loaded, |read| TierRead {
        has_policy_content: read.has_policy_content,
        only_substitutes: read.only_substitutes,
        floored: read.floored,
        ..loaded(read.settings)
    })
}

fn not_loaded() -> TierRead {
    TierRead {
        settings: None,
        ..TierRead::did_not_load(false)
    }
}

#[cfg(test)]
#[path = "settings_managed_reader_tests.rs"]
mod tests;
