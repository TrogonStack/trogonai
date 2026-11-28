use serde_json::Value;

use crate::caller_reads::CallerReads;
use crate::grant_entry::GrantEntry;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

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

fn bad(field: &str, why: &str) -> WindowsSandboxError {
    WindowsSandboxError::for_subcommand(
        Code::FenceHelperBadJson,
        format!("fence-helper profile: `{field}` {why}"),
        "profile",
    )
}

/// A `[{path, reason}]` list as `fence-helper` reports it. An absent or null list is empty;
/// anything else that is not a list of objects naming a `path` is refused, because a row
/// dropped here is a problem the caller never hears about.
pub(crate) fn items(field: &str, v: Option<Value>) -> Result<Vec<ProfileGrantItem>, WindowsSandboxError> {
    let rows = match v {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(rows)) => rows,
        Some(_) => return Err(bad(field, "is not a list")),
    };
    rows.iter()
        .map(|row| {
            let path = row
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| bad(field, "has a row without a text `path`"))?
                .to_string();
            let text = |k: &str| row.get(k).and_then(Value::as_str).map(str::to_string);
            Ok(ProfileGrantItem {
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
