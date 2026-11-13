use std::{path::Path, sync::Arc};

use super::{build, reject_windows_metacharacters};
use crate::{
    ArgvStyle, ClaudeAgentOptions, DangerousPermissionSkip, DebugOutput, HookEvents, PermissionDecision,
    PermissionPrompts, SdkBeta, Skills, TaskBudget, ThinkingConfig, ThinkingDisplay, ToolPermissionRequest, ToolRule,
};

fn render(options: &ClaudeAgentOptions, derived: Option<&Path>) -> Vec<String> {
    build(options, derived)
        .unwrap_or_default()
        .iter()
        .map(|token| token.to_string_lossy().into_owned())
        .collect()
}

fn failure(options: &ClaudeAgentOptions) -> String {
    build(options, None).map_or_else(|error| error.to_string(), |_| String::new())
}

fn handler() -> Arc<dyn crate::ToolPermissionHandler> {
    Arc::new(|_request: ToolPermissionRequest| async { PermissionDecision::allow() })
}

fn everything(style: ArgvStyle) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        argv_style: style,
        thinking: Some(ThinkingConfig::Enabled {
            budget_tokens: Some(2048),
            display: Some(ThinkingDisplay::Omitted),
        }),
        task_budget: Some(TaskBudget { total: 9000 }),
        betas: vec![SdkBeta::Context1m],
        fallback_model: Some("fallback".into()),
        model: Some("main".into()),
        permission_prompt_tool_name: Some("mcp__host__prompt".into()),
        permission_prompts: Some(PermissionPrompts::Host),
        channels: vec!["one".into(), "two".into()],
        debug: DebugOutput::On,
        debug_file: Some("/log/debug.txt".into()),
        include_hook_events: HookEvents::Include,
        session_id: Some("sid".into()),
        resume_session_at: Some("msg-1".into()),
        resume_drops_turn: Some("".into()),
        allow_dangerously_skip_permissions: DangerousPermissionSkip::Allow,
        project_config_root: Some("/root".into()),
        skills: Some(Skills::Named(vec!["alpha".into()])),
        ..ClaudeAgentOptions::default()
    }
}

#[test]
fn typescript_orders_every_runtime_option_like_sdk_mjs() {
    assert_eq!(
        render(&everything(ArgvStyle::TypeScript), None),
        [
            "--output-format",
            "stream-json",
            "--verbose",
            "--input-format",
            "stream-json",
            "--max-thinking-tokens=2048",
            "--thinking-display=omitted",
            "--task-budget=9000",
            "--model=main",
            "--betas=context-1m-2025-08-07",
            "--debug-file=/log/debug.txt",
            "--permission-prompt-tool=mcp__host__prompt",
            "--permission-prompts=host",
            "--channels=one",
            "--channels=two",
            "--allowedTools=Skill(alpha)",
            "--allow-dangerously-skip-permissions",
            "--fallback-model=fallback",
            "--include-hook-events",
            "--project-config-root=/root",
            "--resume-session-at=msg-1",
            "--resume-drops-turn=",
            "--session-id=sid",
        ]
    );
}

#[test]
fn python_orders_every_runtime_option_like_subprocess_cli() {
    assert_eq!(
        render(&everything(ArgvStyle::Python), None),
        [
            "--output-format",
            "stream-json",
            "--verbose",
            "--system-prompt",
            "",
            "--allowedTools",
            "Skill(alpha)",
            "--task-budget",
            "9000",
            "--model",
            "main",
            "--fallback-model",
            "fallback",
            "--betas",
            "context-1m-2025-08-07",
            "--permission-prompt-tool",
            "mcp__host__prompt",
            "--session-id=sid",
            "--include-hook-events",
            "--resume-session-at=msg-1",
            "--resume-drops-turn=",
            "--setting-sources=user,project",
            "--max-thinking-tokens",
            "2048",
            "--thinking-display",
            "omitted",
            "--input-format",
            "stream-json",
        ]
    );
}

#[test]
fn typescript_thinking_forms() {
    let thinking = |config| ClaudeAgentOptions {
        thinking: Some(config),
        ..ClaudeAgentOptions::default()
    };
    let head = |options: &ClaudeAgentOptions| render(options, None)[5..].to_vec();
    assert_eq!(
        head(&thinking(ThinkingConfig::Adaptive { display: None })),
        ["--thinking=adaptive"]
    );
    assert_eq!(
        head(&thinking(ThinkingConfig::Adaptive {
            display: Some(ThinkingDisplay::Summarized)
        })),
        ["--thinking=adaptive", "--thinking-display=summarized"]
    );
    assert_eq!(
        head(&thinking(ThinkingConfig::Enabled {
            budget_tokens: None,
            display: Some(ThinkingDisplay::Omitted)
        })),
        ["--thinking=adaptive", "--thinking-display=omitted"]
    );
    assert_eq!(head(&thinking(ThinkingConfig::Disabled)), ["--thinking=disabled"]);
    let legacy = |tokens| ClaudeAgentOptions {
        max_thinking_tokens: Some(tokens),
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(head(&legacy(0)), ["--thinking=disabled"]);
    assert_eq!(head(&legacy(500)), ["--max-thinking-tokens=500"]);
    let both = ClaudeAgentOptions {
        thinking: Some(ThinkingConfig::Disabled),
        max_thinking_tokens: Some(500),
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(head(&both), ["--thinking=disabled"]);
}

#[test]
fn python_thinking_forms() {
    let options = |thinking, legacy| ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        thinking,
        max_thinking_tokens: legacy,
        ..ClaudeAgentOptions::default()
    };
    let middle = |options: &ClaudeAgentOptions| {
        let all = render(options, None);
        all[5..all.len() - 2].to_vec()
    };
    assert_eq!(
        middle(&options(Some(ThinkingConfig::Adaptive { display: None }), None)),
        ["--thinking", "adaptive"]
    );
    assert_eq!(
        middle(&options(Some(ThinkingConfig::Disabled), Some(7))),
        ["--thinking", "disabled"]
    );
    assert_eq!(middle(&options(None, Some(0))), ["--max-thinking-tokens", "0"]);
    let missing = options(
        Some(ThinkingConfig::Enabled {
            budget_tokens: None,
            display: None,
        }),
        None,
    );
    assert_eq!(failure(&missing), "'budget_tokens'");
}

#[test]
fn typescript_debug_flags() {
    let base = ClaudeAgentOptions::default();
    let tail = |options: &ClaudeAgentOptions, derived: Option<&Path>| render(options, derived)[5..].to_vec();
    let derived = Path::new("/d/sdk-1.txt");
    assert_eq!(tail(&base, Some(derived)), ["--debug-file=/d/sdk-1.txt"]);
    let debug = ClaudeAgentOptions {
        debug: DebugOutput::On,
        ..base.clone()
    };
    assert_eq!(tail(&debug, Some(derived)), ["--debug", "--debug-file=/d/sdk-1.txt"]);
    let file = ClaudeAgentOptions {
        debug: DebugOutput::On,
        debug_file: Some("/f".into()),
        ..base.clone()
    };
    assert_eq!(tail(&file, Some(derived)), ["--debug-file=/f"]);
    let empty = ClaudeAgentOptions {
        debug_file: Some("".into()),
        ..base
    };
    assert_eq!(tail(&empty, Some(derived)), ["--debug-file=/d/sdk-1.txt"]);
}

#[test]
fn typescript_only_options_are_absent_from_the_python_command() {
    let all = render(&everything(ArgvStyle::Python), None);
    for flag in [
        "--debug-file",
        "--debug",
        "--permission-prompts",
        "--channels",
        "--allow-dangerously-skip-permissions",
        "--project-config-root",
    ] {
        assert!(!all.iter().any(|token| token.starts_with(flag)), "{flag}");
    }
}

#[test]
fn permission_prompt_tool_follows_the_callback_in_both_dialects() {
    for style in [ArgvStyle::TypeScript, ArgvStyle::Python] {
        let options = ClaudeAgentOptions {
            argv_style: style,
            permission_handler: Some(handler()),
            ..ClaudeAgentOptions::default()
        };
        let all = render(&options, None);
        let stdio = match style {
            ArgvStyle::TypeScript => all.contains(&"--permission-prompt-tool=stdio".to_owned()),
            ArgvStyle::Python => all.windows(2).any(|pair| pair == ["--permission-prompt-tool", "stdio"]),
        };
        assert!(stdio);
    }
}

#[test]
fn a_callback_with_a_prompt_tool_name_is_rejected_with_each_dialects_text() {
    let mut options = ClaudeAgentOptions {
        permission_handler: Some(handler()),
        permission_prompt_tool_name: Some("tool".into()),
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(
        failure(&options),
        "canUseTool callback cannot be used with permissionPromptToolName. Please use one or the other."
    );
    options.argv_style = ArgvStyle::Python;
    assert_eq!(
        failure(&options),
        "can_use_tool callback cannot be used with permission_prompt_tool_name. Please use one or the other."
    );
    options.permission_prompt_tool_name = Some(String::new());
    assert!(build(&options, None).is_ok());
}

#[test]
fn typescript_rejects_a_fallback_model_equal_to_the_model() {
    let options = ClaudeAgentOptions {
        model: Some("same".into()),
        fallback_model: Some("same".into()),
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(
        failure(&options),
        "Fallback model cannot be the same as the main model. Please specify a different model for fallbackModel option."
    );
    let python = ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        ..options
    };
    assert!(build(&python, None).is_ok());
}

#[test]
fn empty_strings_are_not_forwarded_except_the_drop_turn_guard() {
    let options = ClaudeAgentOptions {
        fallback_model: Some("".into()),
        session_id: Some("".into()),
        resume_session_at: Some("".into()),
        resume_drops_turn: Some("".into()),
        permission_prompt_tool_name: Some("".into()),
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(render(&options, None)[5..], ["--resume-drops-turn="]);
}

#[test]
fn windows_metacharacters_are_listed_sorted_with_python_reprs() {
    assert!(reject_windows_metacharacters("resume", "a&b", false).is_ok());
    assert!(reject_windows_metacharacters("resume", "plain", true).is_ok());
    let error = reject_windows_metacharacters("session_id", "x|y&\"\n&", true);
    assert_eq!(
        error.map_err(|error| error.to_string()),
        Err(
            "session_id value 'x|y&\"\\n&' contains characters that are unsafe to pass on a Windows command line: ['\\n', '\"', '&', '|']"
                .to_owned()
        )
    );
}

#[test]
fn skills_all_adds_the_bare_rule_after_the_callers_tools() {
    let options = ClaudeAgentOptions {
        allowed_tools: vec![ToolRule::from("Read")],
        skills: Some(Skills::All),
        ..ClaudeAgentOptions::default()
    };
    assert!(render(&options, None).contains(&"--allowedTools=Read,Skill".to_owned()));
}

#[test]
fn skill_name_errors_surface_before_any_flag_is_built() {
    let options = ClaudeAgentOptions {
        skills: Some(Skills::Named(vec!["*".into()])),
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(
        failure(&options),
        "Invalid skill name '*': use skills: 'all' to enable every skill."
    );
}
