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
