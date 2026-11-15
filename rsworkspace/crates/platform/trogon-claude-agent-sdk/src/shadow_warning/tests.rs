use std::sync::Arc;

use super::can_use_tool_shadow_warning;
use crate::{
    ArgvStyle, ClaudeAgentOptions, PermissionDecision, PermissionMode, Skills, ToolPermissionRequest, ToolRule,
};

fn options(style: ArgvStyle, mode: Option<PermissionMode>, allowed: &[&str], callback: bool) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        argv_style: style,
        permission_mode: mode,
        allowed_tools: allowed.iter().map(|rule| ToolRule::from(*rule)).collect(),
        permission_handler: callback.then(|| {
            let handler: Arc<dyn crate::ToolPermissionHandler> =
                Arc::new(|_request: ToolPermissionRequest| async { PermissionDecision::allow() });
            handler
        }),
        ..ClaudeAgentOptions::default()
    }
}

#[test]
fn nothing_is_reported_without_a_callback_or_a_shadow() {
    assert!(can_use_tool_shadow_warning(&options(ArgvStyle::TypeScript, None, &["Read"], false)).is_none());
    assert!(can_use_tool_shadow_warning(&options(ArgvStyle::TypeScript, None, &["Bash(ls:*)"], true)).is_none());
    assert!(
        can_use_tool_shadow_warning(&options(ArgvStyle::Python, None, &["Bash(ls:*)", "(x)", " "], true)).is_none()
    );
}

#[test]
fn bypass_permissions_is_reported_with_each_dialects_text() {
    let mode = Some(PermissionMode::BypassPermissions);
    assert_eq!(
        can_use_tool_shadow_warning(&options(ArgvStyle::TypeScript, mode.clone(), &[], true)).as_deref(),
        Some(
            "canUseTool will not be invoked: permissionMode 'bypassPermissions' auto-approves every tool call (except explicit deny rules) before the callback is consulted. To gate every tool call, use a PreToolUse hook instead."
        )
    );
    assert_eq!(
        can_use_tool_shadow_warning(&options(ArgvStyle::Python, mode, &[], true)).as_deref(),
        Some(
            "can_use_tool will not be invoked: permission_mode 'bypassPermissions' auto-approves every tool call (except explicit deny rules) before the callback is consulted. To gate every tool call, use a PreToolUse hook instead."
        )
    );
}

#[test]
fn typescript_lists_bare_entries_and_python_resolves_whole_tool_specifiers() {
    let allowed = ["Read", "Read()", "Glob(*)", "Bash(ls:*)"];
    let typescript = can_use_tool_shadow_warning(&options(ArgvStyle::TypeScript, None, &allowed, true));
    assert!(
        typescript.is_some_and(|text| text.starts_with("canUseTool will not be invoked for: Read. Bare allowedTools"))
    );
    let python = can_use_tool_shadow_warning(&options(ArgvStyle::Python, None, &allowed, true));
    assert!(
        python
            .is_some_and(|text| text.starts_with("can_use_tool will not be invoked for: Read, Glob. An allowed_tools"))
    );
}

#[test]
fn only_python_counts_the_skill_tool_that_skills_all_adds() {
    let mut python = options(ArgvStyle::Python, None, &[], true);
    python.skills = Some(Skills::All);
    assert!(can_use_tool_shadow_warning(&python).is_some_and(|text| text.contains("for: Skill.")));
    let mut typescript = options(ArgvStyle::TypeScript, None, &[], true);
    typescript.skills = Some(Skills::All);
    assert!(can_use_tool_shadow_warning(&typescript).is_none());
}
