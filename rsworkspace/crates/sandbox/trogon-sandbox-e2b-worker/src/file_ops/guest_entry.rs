use serde::Deserialize;
use serde_json::Value;
use trogon_sandbox_contract::{FileKind, FileStat};

use super::guest_entry_error::GuestEntryError;

/// One filesystem entry as envd reports it.
#[derive(Deserialize)]
pub(super) struct GuestEntry {
    name: String,
    path: String,
    #[serde(rename = "type", default)]
    kind: Value,
    #[serde(default)]
    size: Value,
    mode: Option<u32>,
    #[serde(rename = "modifiedTime", alias = "modified_time")]
    modified_time: Option<String>,
    #[serde(rename = "symlinkTarget", alias = "symlink_target")]
    symlink_target: Option<String>,
    #[serde(rename = "isSymlink", alias = "is_symlink", default)]
    is_symlink: bool,
}

impl GuestEntry {
    pub(super) fn parse(value: Value) -> Result<FileStat, GuestEntryError> {
        let entry: Self = serde_json::from_value(value)?;
        let size = match entry.size {
            Value::Null => 0,
            Value::String(text) => text.parse().map_err(|_| GuestEntryError::Size)?,
            Value::Number(number) => number.as_u64().ok_or(GuestEntryError::Size)?,
            _ => return Err(GuestEntryError::Size),
        };
        let modified_unix_ms = entry
            .modified_time
            .map(|text| chrono::DateTime::parse_from_rfc3339(&text).map(|time| time.timestamp_millis()))
            .transpose()
            .map_err(|_| GuestEntryError::Timestamp)?;
        let kind = if entry.is_symlink {
            FileKind::Symlink
        } else {
            match entry.kind.as_str() {
                Some("FILE_TYPE_FILE") => FileKind::File,
                Some("FILE_TYPE_DIRECTORY") => FileKind::Directory,
                Some("FILE_TYPE_SYMLINK") => FileKind::Symlink,
                _ => FileKind::Other,
            }
        };
        Ok(FileStat {
            path: entry.path,
            name: entry.name,
            kind,
            size,
            mode: entry.mode,
            modified_unix_ms,
            symlink_target: entry.symlink_target,
        })
    }
}
