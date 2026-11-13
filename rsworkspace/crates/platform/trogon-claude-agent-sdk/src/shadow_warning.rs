//! The advisory both SDKs raise when a `canUseTool` callback cannot be reached because an allow rule or the
//! permission mode approves tool calls first (`Ltt` in `sdk.mjs`, `_get_can_use_tool_shadowed_warning` in
//! `types.py`). Advisory only: the connection proceeds.

use crate::{ClaudeAgentOptions, OptionError, Skills, process_options::ArgvStyle, skills::is_python_whitespace};

/// Python `_whole_tool_allowed`: the tool an entry allows outright, when it has no specifier or an empty or lone
/// wildcard one.
fn whole_tool_allowed(entry: &str) -> Option<&str> {
    if entry.trim_matches(is_python_whitespace).is_empty() {
        return None;
    }
    let Some(open) = entry.find('(') else {
        return Some(entry);
    };
    if open == 0 || !entry.ends_with(')') {
        return None;
    }
    let specifier = &entry[open + 1..entry.len() - 1];
    matches!(specifier, "" | "*").then(|| &entry[..open])
}

/// The warning text for these options, or `None` when `canUseTool` is not set or nothing shadows it.
pub(crate) fn can_use_tool_shadow_warning(options: &ClaudeAgentOptions) -> Option<String> {
    options.permission_handler.as_ref()?;
    let bypass = options
        .permission_mode
        .as_ref()
        .is_some_and(|mode| mode.as_str() == "bypassPermissions");
    let mut allowed: Vec<&str> = options.allowed_tools.iter().map(|rule| rule.as_str()).collect();
    match options.argv_style {
        ArgvStyle::TypeScript => typescript(bypass, &allowed),
        ArgvStyle::Python => {
            // `skills="all"` makes the transport append a bare `Skill`, which shadows the callback like a
            // hand-written entry. Named skills append specifiers, which do not.
            if options.skills == Some(Skills::All) && !allowed.contains(&"Skill") {
                allowed.push("Skill");
            }
            python(bypass, &allowed)
        }
    }
}

fn typescript(bypass: bool, allowed: &[&str]) -> Option<String> {
    if bypass {
        return Some(
            "canUseTool will not be invoked: permissionMode 'bypassPermissions' auto-approves every tool call (except explicit deny rules) before the callback is consulted. To gate every tool call, use a PreToolUse hook instead."
                .to_owned(),
        );
    }
    let bare: Vec<&str> = allowed
        .iter()
        .copied()
        .filter(|rule| !rule.is_empty() && !rule.contains('('))
        .collect();
    if bare.is_empty() {
        return None;
    }
    Some(format!(
        "canUseTool will not be invoked for: {}. Bare allowedTools entries auto-approve the whole tool before the callback is consulted. To gate every tool call, use a PreToolUse hook; or remove the bare names from allowedTools so they fall through to canUseTool. Allow rules from settings files can also shadow the callback but are not visible here.",
        bare.join(", ")
    ))
}

fn python(bypass: bool, allowed: &[&str]) -> Option<String> {
    if bypass {
        return Some(
            "can_use_tool will not be invoked: permission_mode 'bypassPermissions' auto-approves every tool call (except explicit deny rules) before the callback is consulted. To gate every tool call, use a PreToolUse hook instead."
                .to_owned(),
        );
    }
    let mut shadowed: Vec<&str> = Vec::new();
    for tool in allowed.iter().filter_map(|entry| whole_tool_allowed(entry)) {
        if !shadowed.contains(&tool) {
            shadowed.push(tool);
        }
    }
    if shadowed.is_empty() {
        return None;
    }
    Some(format!(
        "can_use_tool will not be invoked for: {}. An allowed_tools entry that allows a whole tool auto-approves it before the callback is consulted. To gate every tool call, use a PreToolUse hook; or narrow the entry so calls fall through to can_use_tool. Allow rules from settings files can also shadow the callback but are not visible here.",
        shadowed.join(", ")
    ))
}

/// Reports the shadow advisory through `tracing`. Advisory only: the connection proceeds. TypeScript prints it
/// with `console.warn` and Python raises a `CanUseToolShadowedWarning`; both are warnings, not failures.
pub(crate) fn emit(options: &ClaudeAgentOptions) {
    if let Some(message) = can_use_tool_shadow_warning(options) {
        tracing::warn!(target: "trogon_claude_agent_sdk::can_use_tool_shadowed", %message);
    }
}

/// `canUseTool` and `permissionPromptToolName` both choose who answers permission prompts, so upstream refuses
/// the pair.
pub(crate) fn reject_handler_with_tool_name(options: &ClaudeAgentOptions) -> Result<(), OptionError> {
    let named = options
        .permission_prompt_tool_name
        .as_deref()
        .is_some_and(|name| !name.is_empty());
    if options.permission_handler.is_none() || !named {
        return Ok(());
    }
    Err(OptionError::new(match options.argv_style {
        ArgvStyle::TypeScript => {
            "canUseTool callback cannot be used with permissionPromptToolName. Please use one or the other."
        }
        ArgvStyle::Python => {
            "can_use_tool callback cannot be used with permission_prompt_tool_name. Please use one or the other."
        }
    }))
}

#[cfg(test)]
mod tests;
