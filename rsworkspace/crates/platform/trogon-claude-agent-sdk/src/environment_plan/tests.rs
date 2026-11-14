use std::ffi::OsString;

use super::{DEFAULT_ENTRYPOINT, Inherited, SDK_VERSION, plan};
use crate::{
    ClaudeAgentOptions, ClaudeCodeMarker, ConfigValue, Environment, EnvironmentMode, FileCheckpointing, TraceContext,
};

fn inherited(pairs: &[(&str, &str)]) -> Inherited {
    pairs
        .iter()
        .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        .collect()
}

fn options(mode: EnvironmentMode, env: &[(&str, &str)]) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        environment_mode: mode,
        env: Environment::from_iter(env.iter().map(|(k, v)| ((*k).to_owned(), ConfigValue::from(*v)))),
        ..ClaudeAgentOptions::default()
    }
}

fn set(plan: &super::EnvironmentPlan, key: &str) -> Option<String> {
    plan.set
        .get(&OsString::from(key))
        .map(|v| v.to_string_lossy().into_owned())
}

#[test]
fn overlay_forces_version_and_lets_options_override_the_entrypoint() {
    let plan = plan(
        &options(
            EnvironmentMode::Overlay,
            &[("CLAUDE_CODE_ENTRYPOINT", "mine"), ("CLAUDE_AGENT_SDK_VERSION", "9")],
        ),
        &inherited(&[]),
    );
    assert_eq!(set(&plan, "CLAUDE_CODE_ENTRYPOINT").as_deref(), Some("mine"));
    assert_eq!(set(&plan, "CLAUDE_AGENT_SDK_VERSION").as_deref(), Some(SDK_VERSION));
    assert!(!plan.clear);
    assert!(plan.remove.contains(&OsString::from("CLAUDECODE")));
    assert!(
        plan.remove
            .contains(&OsString::from("CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP"))
    );
}

#[test]
fn replace_clears_and_only_defaults_what_options_env_lacks() {
    let plan = plan(
        &options(
            EnvironmentMode::Replace,
            &[
                ("CLAUDE_AGENT_SDK_VERSION", "9"),
                ("NODE_OPTIONS", "x"),
                ("DEBUG", "y"),
                ("CLAUDE_CODE_ENTRYPOINT", ""),
            ],
        ),
        &inherited(&[("DEBUG_CLAUDE_AGENT_SDK", "true")]),
    );
    assert!(plan.clear);
    assert_eq!(set(&plan, "CLAUDE_AGENT_SDK_VERSION").as_deref(), Some("9"));
    // An empty entrypoint is falsy upstream, so the default replaces it.
    assert_eq!(
        set(&plan, "CLAUDE_CODE_ENTRYPOINT").as_deref(),
        Some(DEFAULT_ENTRYPOINT)
    );
    assert_eq!(set(&plan, "NODE_OPTIONS"), None);
    assert_eq!(set(&plan, "DEBUG").as_deref(), Some("1"));
    assert!(!plan.remove.contains(&OsString::from("CLAUDECODE")));
}

#[test]
fn inherit_keeps_an_inherited_entrypoint_and_drops_node_options_and_debug() {
    let kept = plan(
        &options(EnvironmentMode::Inherit, &[]),
        &inherited(&[("CLAUDE_CODE_ENTRYPOINT", "outer")]),
    );
    assert_eq!(set(&kept, "CLAUDE_CODE_ENTRYPOINT"), None);
    assert!(kept.remove.contains(&OsString::from("NODE_OPTIONS")));
    assert!(kept.remove.contains(&OsString::from("DEBUG")));
    let defaulted = plan(&options(EnvironmentMode::Inherit, &[]), &inherited(&[]));
    assert_eq!(
        set(&defaulted, "CLAUDE_CODE_ENTRYPOINT").as_deref(),
        Some(DEFAULT_ENTRYPOINT)
    );
}

#[test]
fn marker_choice_overrides_the_mode_default() {
    let mut keep = options(EnvironmentMode::Overlay, &[]);
    keep.claude_code_marker = ClaudeCodeMarker::Keep;
    assert!(
        !plan(&keep, &inherited(&[]))
            .remove
            .contains(&OsString::from("CLAUDECODE"))
    );
    let mut remove = options(EnvironmentMode::Replace, &[]);
    remove.claude_code_marker = ClaudeCodeMarker::Remove;
    assert!(
        plan(&remove, &inherited(&[]))
            .remove
            .contains(&OsString::from("CLAUDECODE"))
    );
}

#[test]
fn reads_session_state_is_added_unless_any_case_variant_exists() {
    let overlay = options(EnvironmentMode::Overlay, &[]);
    assert_eq!(
        set(&plan(&overlay, &inherited(&[])), "CLAUDE_CODE_SDK_READS_SESSION_STATE").as_deref(),
        Some("1")
    );
    assert_eq!(
        set(
            &plan(&overlay, &inherited(&[("claude_code_sdk_reads_session_state", "0")])),
            "CLAUDE_CODE_SDK_READS_SESSION_STATE"
        ),
        None
    );
    let configured = options(
        EnvironmentMode::Overlay,
        &[("Claude_Code_Sdk_Reads_Session_State", "0")],
    );
    assert_eq!(
        set(
            &plan(&configured, &inherited(&[])),
            "CLAUDE_CODE_SDK_READS_SESSION_STATE"
        ),
        None
    );
}

#[test]
fn replace_mode_ignores_the_inherited_session_state_variable() {
    let replace = options(EnvironmentMode::Replace, &[]);
    assert_eq!(
        set(
            &plan(&replace, &inherited(&[("CLAUDE_CODE_SDK_READS_SESSION_STATE", "0")])),
            "CLAUDE_CODE_SDK_READS_SESSION_STATE"
        )
        .as_deref(),
        Some("1")
    );
}

#[test]
fn trace_context_overrides_inherited_values_unless_options_env_sets_them() {
    let mut traced = options(EnvironmentMode::Overlay, &[]);
    traced.trace_context = Some(TraceContext {
        traceparent: "00-a-b-01".into(),
        tracestate: None,
    });
    let planned = plan(&traced, &inherited(&[]));
    assert_eq!(set(&planned, "TRACEPARENT").as_deref(), Some("00-a-b-01"));
    // A fresh traceparent scrubs a stale inherited tracestate.
    assert!(planned.remove.contains(&OsString::from("TRACESTATE")));

    let mut explicit = options(EnvironmentMode::Overlay, &[("TRACEPARENT", "mine")]);
    explicit.trace_context = traced.trace_context.clone();
    assert_eq!(
        set(&plan(&explicit, &inherited(&[])), "TRACEPARENT").as_deref(),
        Some("mine")
    );
}

#[test]
fn pwd_and_checkpointing_are_overlay_and_option_driven() {
    let mut overlay = options(EnvironmentMode::Overlay, &[]);
    overlay.cwd = Some("/work".into());
    overlay.file_checkpointing = FileCheckpointing::Enabled;
    let planned = plan(&overlay, &inherited(&[]));
    assert_eq!(set(&planned, "PWD").as_deref(), Some("/work"));
    assert_eq!(
        set(&planned, "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING").as_deref(),
        Some("true")
    );
    let mut replace = options(EnvironmentMode::Replace, &[]);
    replace.cwd = Some("/work".into());
    assert_eq!(set(&plan(&replace, &inherited(&[])), "PWD"), None);
}

#[test]
fn inherit_deletes_the_agent_progress_summaries_variable_in_any_case_and_never_sets_it() {
    let inherit = plan(
        &options(EnvironmentMode::Inherit, &[]),
        &inherited(&[
            ("CLAUDE_CODE_AGENT_PROGRESS_SUMMARIES", "1"),
            ("claude_code_agent_progress_summaries", "1"),
        ]),
    );
    assert!(
        inherit
            .remove
            .contains(&OsString::from("CLAUDE_CODE_AGENT_PROGRESS_SUMMARIES"))
    );
    assert!(
        inherit
            .remove
            .contains(&OsString::from("claude_code_agent_progress_summaries"))
    );
    assert_eq!(set(&inherit, "CLAUDE_CODE_AGENT_PROGRESS_SUMMARIES"), None);
    let replace = plan(
        &options(EnvironmentMode::Replace, &[]),
        &inherited(&[("CLAUDE_CODE_AGENT_PROGRESS_SUMMARIES", "1")]),
    );
    assert!(
        replace.remove.is_empty()
            || !replace
                .remove
                .contains(&OsString::from("CLAUDE_CODE_AGENT_PROGRESS_SUMMARIES"))
    );
}

fn with_tool_config(mode: EnvironmentMode, env: &[(&str, &str)]) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        tool_config: crate::ToolConfig {
            ask_user_question: crate::AskUserQuestionConfig {
                preview_format: Some(crate::QuestionPreviewFormat::Html),
                extended_questions: true,
                optional_descriptions: false,
            },
        },
        ..options(mode, env)
    }
}

#[test]
fn tool_config_sets_its_variables_and_cleans_an_inherited_environment_in_any_case() {
    let plan = plan(
        &with_tool_config(EnvironmentMode::Inherit, &[]),
        &inherited(&[
            ("claude_code_question_extended", "old"),
            ("Claude_Code_Question_Optional_Descriptions", "x"),
            ("CLAUDE_CODE_AGENT_PROGRESS_SUMMARIES", "1"),
        ]),
    );
    assert_eq!(
        set(&plan, "CLAUDE_CODE_QUESTION_PREVIEW_FORMAT").as_deref(),
        Some("html")
    );
    assert_eq!(set(&plan, "CLAUDE_CODE_QUESTION_EXTENDED").as_deref(), Some("1"));
    // A truthy value only writes the exact name; another spelling of it stays.
    assert!(!plan.remove.contains(&OsString::from("claude_code_question_extended")));
    assert!(
        plan.remove
            .contains(&OsString::from("Claude_Code_Question_Optional_Descriptions"))
    );
    assert!(
        plan.remove
            .contains(&OsString::from("CLAUDE_CODE_AGENT_PROGRESS_SUMMARIES"))
    );
    assert_eq!(set(&plan, "CLAUDE_CODE_QUESTION_OPTIONAL_DESCRIPTIONS"), None);
}

#[test]
fn tool_config_with_an_explicit_environment_never_deletes() {
    let plan = plan(
        &with_tool_config(EnvironmentMode::Replace, &[("claude_code_question_extended", "q")]),
        &inherited(&[]),
    );
    assert_eq!(set(&plan, "CLAUDE_CODE_QUESTION_EXTENDED").as_deref(), Some("1"));
    assert_eq!(set(&plan, "claude_code_question_extended").as_deref(), Some("q"));
    assert!(
        !plan
            .remove
            .contains(&OsString::from("CLAUDE_CODE_AGENT_PROGRESS_SUMMARIES"))
    );
}

#[test]
fn an_empty_tool_config_only_cleans_in_inherit_mode() {
    let inherit = plan(&options(EnvironmentMode::Inherit, &[]), &inherited(&[]));
    for name in [
        "CLAUDE_CODE_QUESTION_EXTENDED",
        "CLAUDE_CODE_QUESTION_OPTIONAL_DESCRIPTIONS",
        "CLAUDE_CODE_AGENT_PROGRESS_SUMMARIES",
    ] {
        assert!(inherit.remove.contains(&OsString::from(name)), "{name}");
    }
    let replace = plan(&options(EnvironmentMode::Replace, &[]), &inherited(&[]));
    assert!(
        !replace
            .remove
            .contains(&OsString::from("CLAUDE_CODE_QUESTION_EXTENDED"))
    );
    let overlay = plan(&options(EnvironmentMode::Overlay, &[]), &inherited(&[]));
    assert!(
        !overlay
            .remove
            .contains(&OsString::from("CLAUDE_CODE_QUESTION_EXTENDED"))
    );
}

#[test]
fn python_dialect_ignores_tool_config() {
    let mut python = with_tool_config(EnvironmentMode::Overlay, &[]);
    python.argv_style = crate::ArgvStyle::Python;
    let plan = plan(&python, &inherited(&[]));
    assert_eq!(set(&plan, "CLAUDE_CODE_QUESTION_PREVIEW_FORMAT"), None);
    assert_eq!(set(&plan, "CLAUDE_CODE_QUESTION_EXTENDED"), None);
}

#[test]
fn token_callbacks_announce_themselves_in_every_typescript_mode() {
    let handler: std::sync::Arc<dyn crate::OAuthTokenHandler> =
        std::sync::Arc::new(|_request: crate::OAuthTokenRequest| async { Ok(crate::OAuthTokenResponse::default()) });
    let host: std::sync::Arc<dyn crate::HostAuthTokenHandler> =
        std::sync::Arc::new(|_request: crate::HostAuthTokenRequest| async { Ok(None) });
    for mode in [
        EnvironmentMode::TypeScript,
        EnvironmentMode::Overlay,
        EnvironmentMode::Replace,
        EnvironmentMode::Inherit,
    ] {
        let bare = plan(&options(mode, &[]), &inherited(&[]));
        assert_eq!(set(&bare, "CLAUDE_CODE_SDK_HAS_OAUTH_REFRESH"), None);
        let mut both = options(mode, &[]);
        both.oauth_token_handler = Some(handler.clone());
        both.host_auth_token_handler = Some(host.clone());
        let plan = plan(&both, &inherited(&[]));
        assert_eq!(set(&plan, "CLAUDE_CODE_SDK_HAS_OAUTH_REFRESH").as_deref(), Some("1"));
        assert_eq!(
            set(&plan, "CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH").as_deref(),
            Some("1")
        );
        both.argv_style = crate::ArgvStyle::Python;
        let python = super::plan(&both, &inherited(&[]));
        assert_eq!(set(&python, "CLAUDE_CODE_SDK_HAS_OAUTH_REFRESH"), None);
        assert_eq!(set(&python, "CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH"), None);
    }
}

#[test]
fn the_typescript_default_replaces_with_a_configured_environment_and_inherits_without_one() {
    assert_eq!(EnvironmentMode::default(), EnvironmentMode::TypeScript);
    let process = inherited(&[("DEBUG_CLAUDE_AGENT_SDK", "1"), ("CLAUDE_CODE_ENTRYPOINT", "outer")]);
    let configured = [("ANTHROPIC_API_KEY", "key"), ("NODE_OPTIONS", "x")];
    assert_eq!(
        plan(&options(EnvironmentMode::TypeScript, &configured), &process),
        plan(&options(EnvironmentMode::Replace, &configured), &process)
    );
    assert!(plan(&options(EnvironmentMode::TypeScript, &configured), &process).clear);
    assert_eq!(
        plan(&options(EnvironmentMode::TypeScript, &[]), &process),
        plan(&options(EnvironmentMode::Inherit, &[]), &process)
    );
    let overlay = options(EnvironmentMode::Overlay, &configured);
    assert_eq!(overlay.environment_mode.resolve(&overlay.env), EnvironmentMode::Overlay);
}

#[test]
fn an_explicit_client_app_is_set_in_every_mode_over_the_configured_env() {
    let configured = [("CLAUDE_AGENT_SDK_CLIENT_APP", "configured/1")];
    for mode in [
        EnvironmentMode::TypeScript,
        EnvironmentMode::Overlay,
        EnvironmentMode::Replace,
        EnvironmentMode::Inherit,
    ] {
        let mut chosen = options(mode, &configured);
        chosen.client_app = Some("explicit/2".to_owned());
        let explicit = plan(&chosen, &inherited(&[]));
        assert_eq!(
            set(&explicit, "CLAUDE_AGENT_SDK_CLIENT_APP").as_deref(),
            Some("explicit/2"),
            "{mode:?}"
        );
        assert_eq!(set(&explicit, "CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP"), None, "{mode:?}");
    }
    let replace = plan(
        &ClaudeAgentOptions {
            client_app: Some("explicit/2".to_owned()),
            ..options(EnvironmentMode::Replace, &[("ANTHROPIC_API_KEY", "key")])
        },
        &inherited(&[]),
    );
    assert!(replace.clear);
    assert_eq!(
        set(&replace, "CLAUDE_AGENT_SDK_CLIENT_APP").as_deref(),
        Some("explicit/2")
    );
    let unset = plan(&options(EnvironmentMode::Replace, &configured), &inherited(&[]));
    assert_eq!(
        set(&unset, "CLAUDE_AGENT_SDK_CLIENT_APP").as_deref(),
        Some("configured/1")
    );
}
