//! The first-init-message version warning of Python `_check_claude_version` (0.2.181).

use crate::constants::{MINIMUM_CLAUDE_CODE_VERSION, VERBATIM_PROMPTS_MINIMUM};

/// Python's `re.match(r"(\d{1,9})\.(\d{1,9})\.(\d{1,9})", version)`: a prefix match.
pub fn parse_version(version: &str) -> Option<[u32; 3]> {
    let mut parts = [0_u32; 3];
    let mut rest = version;
    for (index, slot) in parts.iter_mut().enumerate() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count().min(9);
        if digits == 0 {
            return None;
        }
        *slot = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        if index < 2 {
            rest = rest.strip_prefix('.')?;
        }
    }
    Some(parts)
}

/// The warning text for a CLI that does not report a parseable version, else `None`.
pub fn unreported_version_warning(version: Option<&str>, cli_path: &str) -> Option<String> {
    // Python formats `str(None)`, which never matches.
    let reported = version.unwrap_or("None");
    parse_version(reported).is_none().then(|| {
        format!(
            "Claude Code at {cli_path} is too old to report its version, and may be unsupported in the Agent SDK. Minimum required version is {MINIMUM_CLAUDE_CODE_VERSION}. Some features may not work correctly."
        )
    })
}

/// The warning for `verbatim_prompts` with a CLI that is older than 2.1.248 or reports no usable version.
pub fn unsupported_verbatim_warning(version: Option<&str>, cli_path: &str) -> Option<String> {
    let supported = version
        .and_then(parse_version)
        .is_some_and(|parsed| parsed >= VERBATIM_PROMPTS_MINIMUM);
    (!supported).then(|| {
        format!(
            "verbatim_prompts is enabled, but Claude Code version {} at {cli_path} ignores it: prompts will still have @path mentions expanded and slash commands dispatched. Claude Code 2.1.248 or later is required.",
            version.filter(|version| !version.is_empty()).unwrap_or("unknown"),
        )
    })
}

/// Skips the check exactly as `CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK` does: any non-empty value disables it.
pub fn skip_requested(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

#[cfg(test)]
mod tests;
