//! Ports the pinned bundle's connector download folder check (`nE` and the helpers it calls).
//!
//! Settings files under a folder named `connector-downloads` are skipped when that folder is the sandbox mount, holds
//! a download in progress, or carries a `.gitignore` that ignores everything. The `.gitignore` path is vetted first,
//! so a link that leads somewhere else is never trusted.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use crate::connector_vetting::{Vetter, is_invisible, is_slash};
use crate::constants::{
    CONNECTOR_CACHE_LIMIT, CONNECTOR_DOWNLOADS_MOUNT, CONNECTOR_DOWNLOADS_NAME, CONNECTOR_GITIGNORE_MARKERS,
    CONNECTOR_GITIGNORE_MAX_BYTES, CONNECTOR_GITIGNORE_READ_BYTES, NODE_PATH_CHAR_COLON as COLON,
    NODE_PATH_CHAR_DOT as DOT, NODE_PATH_CHAR_FORWARD_SLASH as SLASH,
};
use crate::node_path::{JsString, NodeError, js, lower};

#[cfg(test)]
#[path = "connector_folder_tests.rs"]
mod tests;

/// The answers `nE` remembers for the life of the process (`tE` upstream), keyed by the path as the caller spelled it.
#[derive(Debug, Default)]
pub(crate) struct ConnectorFolders {
    cache: Mutex<HashMap<JsString, bool>>,
}

impl ConnectorFolders {
    /// `nE`: whether `path` sits under a connector download folder the settings walk skips.
    pub(crate) fn is_ignored(&self, vetter: &Vetter<'_>, path: &[u16]) -> Result<bool, NodeError> {
        if let Some(known) = self.cache.lock().unwrap_or_else(PoisonError::into_inner).get(path) {
            return Ok(*known);
        }
        let resolved = vetter.resolve(&[path])?;
        let parts: Vec<&[u16]> = resolved.split(|unit| is_slash(*unit)).collect();
        let mut ignored = false;
        for (index, part) in parts.iter().enumerate() {
            if ignored {
                break;
            }
            if !is_connector_name(part) {
                continue;
            }
            let folder = parts.get(..=index).unwrap_or_default().join(&SLASH);
            ignored = folder == js(CONNECTOR_DOWNLOADS_MOUNT)
                || folder_ignored(vetter, &[folder, js("/.gitignore")].concat());
        }
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.len() >= CONNECTOR_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(path.to_vec(), ignored);
        Ok(ignored)
    }
}

/// `n7e`: whether the folder holding `gitignore` is skipped; anything that fails along the way counts as skipped.
pub(crate) fn folder_ignored(vetter: &Vetter<'_>, gitignore: &[u16]) -> bool {
    gitignore_ignores(vetter, gitignore).unwrap_or(true)
}

fn gitignore_ignores(vetter: &Vetter<'_>, gitignore: &[u16]) -> Option<bool> {
    let host = vetter.host();
    let vetted = vetter.vet(gitignore).ok()?;
    let names = host.read_dir_names(parent_of(gitignore)).ok()?;
    if names.iter().any(|name| is_download_temp_name(name)) {
        return Some(true);
    }
    if vetted.absent_at.is_some() {
        return Some(false);
    }
    let stat = host.lstat(gitignore).ok()?;
    if !stat.file || stat.size > CONNECTOR_GITIGNORE_MAX_BYTES {
        return Some(false);
    }
    let head = host.read_head(gitignore, CONNECTOR_GITIGNORE_READ_BYTES).ok()?;
    Some(CONNECTOR_GITIGNORE_MARKERS.contains(&head.as_slice()))
}

/// `Q8e`: drops the last separator and everything after it.
fn parent_of(path: &[u16]) -> &[u16] {
    path.iter()
        .rposition(|unit| is_slash(*unit))
        .and_then(|index| path.get(..index))
        .unwrap_or(path)
}

fn is_lower_hex(unit: u16) -> bool {
    matches!(u8::try_from(unit), Ok(b'0'..=b'9' | b'a'..=b'f'))
}

fn is_ascii_digit(unit: u16) -> bool {
    matches!(u8::try_from(unit), Ok(b'0'..=b'9'))
}

/// `[^./\\]`.
fn is_name_unit(unit: u16) -> bool {
    unit != DOT && !is_slash(unit)
}

/// `(\.[^./\\]*)?` matching all of `text`.
fn is_optional_extension(text: &[u16]) -> bool {
    match text.split_first() {
        None => true,
        Some((first, rest)) => *first == DOT && rest.iter().all(|unit| is_name_unit(*unit)),
    }
}

/// The text after `download-[0-9a-f]{8}(-\d+)?`.
fn after_download_id(text: &[u16]) -> Option<&[u16]> {
    let (id, rest) = text.split_at_checked(8)?;
    if !id.iter().all(|unit| is_lower_hex(*unit)) {
        return None;
    }
    if let Some(digits) = rest.strip_prefix(&[u16::from(b'-')][..]) {
        let count = digits.iter().take_while(|unit| is_ascii_digit(**unit)).count();
        if count > 0 {
            return digits.get(count..);
        }
    }
    Some(rest)
}

/// `e7e`: a download in progress (`download-<id>[.ext]`, or `.download-<id>[.ext].from.json` and `.part`).
fn is_download_temp_name(name: &[u16]) -> bool {
    if let Some(rest) = name.strip_prefix(js("download-").as_slice()) {
        return after_download_id(rest).is_some_and(is_optional_extension);
    }
    let Some(rest) = name.strip_prefix(js(".download-").as_slice()) else {
        return false;
    };
    let Some(rest) = after_download_id(rest) else {
        return false;
    };
    [".from.json", ".part"].iter().any(|suffix| {
        rest.strip_suffix(js(suffix).as_slice())
            .is_some_and(is_optional_extension)
    })
}

fn is_line_terminator(unit: u16) -> bool {
    matches!(unit, 0x0a | 0x0d | 0x2028 | 0x2029)
}

/// `rXe` as the connector check calls it, with the NTFS stream suffix stripped: the folder name the check compares.
pub(crate) fn normalized_name(name: &[u16]) -> JsString {
    let lowered: JsString = lower(name)
        .into_iter()
        .map(|unit| match unit {
            0x131 => u16::from(b'i'),
            0x17f => u16::from(b's'),
            _ => unit,
        })
        .collect();
    let mut visible: JsString = lowered.iter().copied().filter(|unit| !is_invisible(*unit)).collect();
    let start = visible
        .iter()
        .rposition(|unit| is_line_terminator(*unit))
        .map_or(0, |index| index + 1);
    if let Some(colon) = visible.iter().skip(start).position(|unit| *unit == COLON) {
        visible.truncate(start + colon);
    }
    let end = visible
        .iter()
        .rposition(|unit| *unit != DOT && *unit != u16::from(b' '))
        .map_or(0, |index| index + 1);
    visible.truncate(end);
    if visible.is_empty() { lowered } else { visible }
}

/// `H5`: the name is `connector-downloads` once normalized.
pub(crate) fn is_connector_name(name: &[u16]) -> bool {
    normalized_name(name) == js(CONNECTOR_DOWNLOADS_NAME)
}

/// `im`: some component of `path` is a connector download folder (`X8e` of `path + "/."`, whose last component is
/// the appended `.`).
pub(crate) fn names_connector_folder(path: &[u16]) -> bool {
    path.split(|unit| is_slash(*unit)).any(is_connector_name)
}
