//! Application labels for `CLAUDE_AGENT_SDK_CLIENT_APP`, matching Python `_client_app.py` (0.2.181).
//!
//! The pattern is `(@scope/)?name(/version)?` with a 128 character cap. Generic runtime names are never labels.

use crate::constants::{GENERIC_NAMES, MAX_CLIENT_APP_LENGTH};

fn name_part(part: &str) -> bool {
    let mut characters = part.chars();
    characters.next().is_some_and(|first| first.is_ascii_alphanumeric())
        && characters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn scope_part(part: &str) -> bool {
    part.strip_prefix('@').is_some_and(|scope| {
        !scope.is_empty()
            && scope
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    })
}

fn version_part(part: &str) -> bool {
    let mut characters = part.chars();
    characters.next().is_some_and(|first| first.is_ascii_alphanumeric())
        && characters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '!' | '+' | '_' | '-'))
}

/// Returns the value unchanged when it is a sendable label. Never a repaired copy.
pub fn normalize_client_app(value: &str) -> Option<&str> {
    if value.chars().count() > MAX_CLIENT_APP_LENGTH {
        return None;
    }
    let parts: Vec<&str> = value.split('/').collect();
    let valid = match parts.as_slice() {
        [name] => name_part(name),
        [first, second] if scope_part(first) => name_part(second),
        [name, version] => name_part(name) && version_part(version),
        [scope, name, version] => scope_part(scope) && name_part(name) && version_part(version),
        _ => false,
    };
    valid.then_some(value)
}

/// Formats `name/version` when sendable, else `name`, and nothing for a generic name.
pub fn format_client_app(name: &str, version: Option<&str>) -> Option<String> {
    if GENERIC_NAMES.contains(&name.to_ascii_lowercase().as_str()) {
        return None;
    }
    let public_version = version.map(|version| version.split('+').next().unwrap_or_default());
    public_version
        .filter(|version| !version.is_empty())
        .and_then(|version| normalize_client_app(&format!("{name}/{version}")).map(str::to_owned))
        .or_else(|| normalize_client_app(name).map(str::to_owned))
}

#[cfg(test)]
mod tests;
