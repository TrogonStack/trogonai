mod constants;

use crate::CommitSha;

use constants::{MAX_NAME_LEN, SHA_PREFIX_LEN};

/// Derives a deterministic, RFC-1123-safe job name (at most 63 characters) from the repo identity and the resolved
/// SHA, so retries against the same commit collide and callers can detect duplicates rather than spinning up ghost
/// jobs. A SHA shorter than eight characters contributes all of itself.
pub fn job_name(owner: &str, repo: &str, sha: &CommitSha) -> String {
    let sha = sha.as_str();
    let sha_prefix = sha.get(..SHA_PREFIX_LEN).unwrap_or(sha);
    let name: String = format!("rs-{owner}-{repo}-{sha_prefix}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(MAX_NAME_LEN)
        .collect();
    name.trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests;
