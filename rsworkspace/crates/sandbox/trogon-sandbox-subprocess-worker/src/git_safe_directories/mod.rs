//! `safe.directory` entries for the payload, passed as `GIT_CONFIG_*`
//! variables. Inside the sandbox another user owns the working tree, so git
//! would refuse it as dubious ownership.

use trogon_sandbox_contract::LiteralPath;

mod constants;

use constants::{COUNT_VAR, KEY, WILDCARD_THRESHOLD};

/// Adds the entries to `env`, continuing the numbering of a `GIT_CONFIG_COUNT`
/// the lease already sets. An explicit `GIT_CONFIG_COUNT=0` opts out, and
/// nothing is added.
pub fn apply(env: &mut Vec<(String, String)>, directories: &[LiteralPath]) {
    if directories.is_empty() {
        return;
    }
    let existing = env
        .iter()
        .rev()
        .find(|(name, _)| name == COUNT_VAR)
        .map(|(_, value)| value.as_str());
    if existing == Some("0") {
        return;
    }
    let start = existing.and_then(|value| value.parse::<usize>().ok()).unwrap_or(0);

    let mut values = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let unique: Vec<&str> = directories
        .iter()
        .map(LiteralPath::as_str)
        .filter(|dir| seen.insert(*dir))
        .collect();
    if unique.len() > WILDCARD_THRESHOLD {
        values.push("*".to_string());
    } else {
        for dir in unique {
            values.push(dir.to_string());
            values.push(if dir.ends_with('/') {
                format!("{dir}*")
            } else {
                format!("{dir}/*")
            });
        }
    }

    env.retain(|(name, _)| name != COUNT_VAR);
    for (offset, value) in values.iter().enumerate() {
        let index = start + offset;
        env.retain(|(name, _)| {
            name != &format!("GIT_CONFIG_KEY_{index}") && name != &format!("GIT_CONFIG_VALUE_{index}")
        });
        env.push((format!("GIT_CONFIG_KEY_{index}"), KEY.to_string()));
        env.push((format!("GIT_CONFIG_VALUE_{index}"), value.clone()));
    }
    env.push((COUNT_VAR.to_string(), (start + values.len()).to_string()));
}

#[cfg(test)]
mod tests;
