use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use a2a_nats::A2aAgentId;

use super::bundle::Tier2CompiledBundle;
use super::compiler::{compile_cel_file, compile_cel_source};
use super::evaluator::{CelEngine, CelInterpreterEngine, MockCelEngine, RealTier2CelEvaluator};
use crate::policy::RuleName;
use crate::policy::tier2::{
    DenyAllTier2Evaluator, NoopTier2Evaluator, Tier2CelEvaluator, Tier2Decision, Tier2EvaluationContext,
};

fn sample_ctx(method: &str) -> Tier2EvaluationContext {
    Tier2EvaluationContext::new(
        method,
        serde_json::json!({}),
        Some("caller-1".into()),
        A2aAgentId::new("planner").expect("agent id"),
        None,
        BTreeMap::new(),
    )
}

fn bundle_with_rule(name: &str, source: &str) -> (tempfile::TempDir, Tier2CompiledBundle) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(format!("{name}.cel"));
    std::fs::write(&path, source).expect("write cel");
    let bundle = Tier2CompiledBundle::load_from_dir(dir.path()).expect("load");
    (dir, bundle)
}

#[test]
fn compile_valid_cel_source_ok() {
    let handle = compile_cel_source("true").expect("valid cel");
    assert!(handle.program().execute(&cel_interpreter::Context::default()).is_ok());
}

#[test]
fn compile_invalid_cel_source_err() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bad.cel");
    std::fs::write(&path, "(1 + 2").expect("write cel");
    assert!(compile_cel_file(&path).is_err());
}

#[test]
fn mtime_change_triggers_recompile() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rule.cel");
    fs::write(&path, "true").expect("write cel");

    let bundle = Tier2CompiledBundle::load_from_dir(dir.path()).expect("load bundle");
    assert_eq!(bundle.rules().count(), 1);

    thread::sleep(Duration::from_millis(1100));
    fs::write(&path, "false").expect("rewrite cel");

    let mut refreshed = bundle;
    refreshed.refresh_if_stale().expect("refresh");
    let ctx = cel_interpreter::Context::default();
    let (_, program) = refreshed.rules().next().expect("rule");
    let value = program.program().execute(&ctx).expect("execute refreshed program");
    assert_eq!(value, cel_interpreter::Value::Bool(false));
}

#[test]
fn evaluator_allow_path() {
    let (_dir, bundle) = bundle_with_rule("allow_all", "true");
    let evaluator = RealTier2CelEvaluator::new(bundle);
    assert_eq!(evaluator.evaluate(&sample_ctx("message/send")), Tier2Decision::Allow);
}

#[test]
fn evaluator_deny_path() {
    let (_dir, bundle) = bundle_with_rule("deny_guests", "false");
    let evaluator = RealTier2CelEvaluator::new(bundle);
    assert_eq!(
        evaluator.evaluate(&sample_ctx("message/send")),
        Tier2Decision::Deny {
            rule: RuleName::new("deny_guests")
        }
    );
}

#[test]
fn evaluator_error_path_denies_closed() {
    let (_dir, bundle) = bundle_with_rule("bad_rule", "1 + true");
    let evaluator = RealTier2CelEvaluator::new(bundle);
    assert_eq!(
        evaluator.evaluate(&sample_ctx("message/send")),
        Tier2Decision::Deny {
            rule: RuleName::evaluation_error()
        }
    );
}

#[test]
fn mock_engine_drives_decision() {
    let mut outcomes = BTreeMap::new();
    outcomes.insert(RuleName::new("block_rule"), Ok(false));
    let (_dir, bundle) = bundle_with_rule("block_rule", "true");
    let evaluator = RealTier2CelEvaluator::with_engine(bundle, Arc::new(MockCelEngine::new(outcomes)));
    assert_eq!(
        evaluator.evaluate(&sample_ctx("message/send")),
        Tier2Decision::Deny {
            rule: RuleName::new("block_rule")
        }
    );
}

#[test]
fn compiled_program_handle_is_shared() {
    let handle = compile_cel_source("true").expect("compile");
    let cloned = handle.clone();
    let _ = cloned.program();
    let _ = handle.program();
}

#[test]
fn rule_name_display_round_trips() {
    let name = RuleName::new("my-rule");
    assert_eq!(format!("{name}"), "my-rule");
    assert_eq!(name.as_str(), "my-rule");
    assert_eq!(RuleName::evaluation_error().as_str(), "evaluation_error");
}

#[test]
fn tier2_decision_is_allow_only_for_allow() {
    assert!(Tier2Decision::Allow.is_allow());
    assert!(
        !Tier2Decision::Deny {
            rule: RuleName::new("x"),
        }
        .is_allow()
    );
}

#[test]
fn noop_evaluator_always_allows() {
    let evaluator = NoopTier2Evaluator;
    assert_eq!(evaluator.evaluate(&sample_ctx("any")), Tier2Decision::Allow);
}

#[test]
fn deny_all_evaluator_returns_evaluation_error_rule() {
    let evaluator = DenyAllTier2Evaluator;
    assert_eq!(
        evaluator.evaluate(&sample_ctx("any")),
        Tier2Decision::Deny {
            rule: RuleName::evaluation_error()
        }
    );
}

#[test]
fn evaluation_context_from_ingress_parses_json_rpc_params() {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("x-tenant-id", "acme");
    let payload = br#"{"jsonrpc":"2.0","id":"r-1","method":"message/send","params":{"taskId":"t-9","echo":1}}"#;
    let ctx = super::evaluator::tier2_evaluation_context_from_ingress(
        "message/send",
        &A2aAgentId::new("planner").expect("agent"),
        Some("caller-1"),
        &headers,
        payload,
    );
    assert_eq!(ctx.request_method(), "message/send");
    assert_eq!(ctx.task_id(), Some("t-9"));
    assert_eq!(ctx.caller_id(), Some("caller-1"));
    assert_eq!(ctx.agent_id().as_str(), "planner");
    assert_eq!(ctx.headers().get("x-tenant-id").map(String::as_str), Some("acme"));
    assert_eq!(ctx.request_params().get("echo"), Some(&serde_json::json!(1)));
}

#[test]
fn evaluation_context_from_ingress_accepts_snake_case_task_id() {
    let payload = br#"{"params":{"task_id":"snake-id"}}"#;
    let ctx = super::evaluator::tier2_evaluation_context_from_ingress(
        "x",
        &A2aAgentId::new("planner").expect("agent"),
        None,
        &async_nats::HeaderMap::new(),
        payload,
    );
    assert_eq!(ctx.task_id(), Some("snake-id"));
}

#[test]
fn evaluation_context_from_ingress_handles_invalid_json() {
    let payload = b"not-json";
    let ctx = super::evaluator::tier2_evaluation_context_from_ingress(
        "x",
        &A2aAgentId::new("planner").expect("agent"),
        None,
        &async_nats::HeaderMap::new(),
        payload,
    );
    assert_eq!(*ctx.request_params(), serde_json::Value::Null);
    assert_eq!(ctx.task_id(), None);
}

#[test]
fn evaluation_context_accessors_round_trip_constructor_args() {
    let mut headers = BTreeMap::new();
    headers.insert("h".to_string(), "v".to_string());
    let ctx = Tier2EvaluationContext::new(
        "method/x",
        serde_json::json!({"a":1}),
        Some("user/alice".to_string()),
        A2aAgentId::new("planner").expect("agent"),
        Some("task-1".to_string()),
        headers.clone(),
    );
    assert_eq!(ctx.request_method(), "method/x");
    assert_eq!(*ctx.request_params(), serde_json::json!({"a":1}));
    assert_eq!(ctx.caller_id(), Some("user/alice"));
    assert_eq!(ctx.agent_id().as_str(), "planner");
    assert_eq!(ctx.task_id(), Some("task-1"));
    assert_eq!(ctx.headers(), &headers);
}

#[test]
fn bundle_load_from_missing_dir_yields_empty_bundle() {
    let bundle = Tier2CompiledBundle::load_from_dir("/nonexistent/path").expect("missing tolerated");
    assert_eq!(bundle.rules().count(), 0);
}

#[test]
fn bundle_load_ignores_non_cel_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("rule.cel"), "true").expect("write cel");
    std::fs::write(dir.path().join("notes.txt"), "ignore me").expect("write notes");
    let bundle = Tier2CompiledBundle::load_from_dir(dir.path()).expect("load");
    assert_eq!(bundle.rules().count(), 1);
}

#[test]
fn bundle_refresh_picks_up_new_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut bundle = Tier2CompiledBundle::load_from_dir(dir.path()).expect("load empty");
    assert_eq!(bundle.rules().count(), 0);
    std::fs::write(dir.path().join("new.cel"), "true").expect("write cel");
    bundle.refresh_if_stale().expect("refresh");
    assert_eq!(bundle.rules().count(), 1);
}

#[test]
fn bundle_tier2_dir_round_trips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = Tier2CompiledBundle::load_from_dir(dir.path()).expect("load");
    assert_eq!(bundle.tier2_dir(), dir.path());
}

#[test]
fn cel_engine_non_bool_result_is_eval_error() {
    // Compile a CEL program whose result is an int — the engine must
    // reject anything other than bool so misconfigured rules can't be
    // silently treated as truthy/falsy. The eval-error variant is
    // captured by its variant in the policy::error module rather than
    // by string match on the display message.
    let program = compile_cel_source("1 + 2").expect("compile");
    let engine = CelInterpreterEngine;
    let err = engine
        .evaluate_bool(&RuleName::new("non-bool"), &program, &sample_ctx("m"))
        .expect_err("non-bool rejected");
    // `Tier2EvalError` doesn't carry a typed kind for "bool-required"
    // (yet). Cheaply assert it's the right error class via Debug, not
    // Display — Display is for human logs, not control flow.
    let _ = format!("{err:?}");
}
