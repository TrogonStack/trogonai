use serde_json::Value;

use crate::caller_reads::CallerReads;
use crate::grant_entry::GrantEntry;

/// One path `fence-helper profile grant` skipped or could not write.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProfileGrantItem {
    pub path: String,
    pub reason: String,
    pub caller_reads: Option<CallerReads>,
    pub cause: Option<String>,
    pub protected_exclusion: Option<bool>,
    pub entry: Option<GrantEntry>,
}

/// A `[{path, reason}]` list as `fence-helper` reports it; anything else is none.
pub(crate) fn items(v: Option<Value>) -> Vec<ProfileGrantItem> {
    let Some(Value::Array(rows)) = v else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let path = row.get("path")?.as_str()?.to_string();
            let text = |k: &str| row.get(k).and_then(Value::as_str).map(str::to_string);
            Some(ProfileGrantItem {
                path,
                reason: text("reason").unwrap_or_default(),
                caller_reads: match row.get("caller_reads") {
                    Some(Value::Bool(true)) => Some(CallerReads::Yes),
                    Some(Value::Bool(false)) => Some(CallerReads::No),
                    Some(Value::String(s)) if s == "unknown" => Some(CallerReads::Unknown),
                    _ => None,
                },
                cause: text("cause"),
                protected_exclusion: row.get("protected_exclusion").and_then(Value::as_bool),
                entry: match row.get("entry").and_then(Value::as_str) {
                    Some("explicit") => Some(GrantEntry::Explicit),
                    Some("inherited") => Some(GrantEntry::Inherited),
                    _ => None,
                },
            })
        })
        .collect()
}
