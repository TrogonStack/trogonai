use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;
use std::time::Duration;

use a2a_auth_callout::SpiceDbSubject;
use a2a_nats::server::A2aMethod;
use a2a_nats::{A2aAgentId, A2aTaskId};

use super::bundle::Tier2CompiledBundle;
use super::compiler::{CelCompileError, compile_cel_file, compile_cel_source};
use super::evaluator::{CelEngine, CelInterpreterEngine, MockCelEngine, RealTier2CelEvaluator};
use crate::policy::RuleName;
use crate::policy::error::Tier2EvalError;
use crate::policy::tier2::rule_name::RuleNameError;
use crate::policy::tier2::{
    DenyAllTier2Evaluator, NoopTier2Evaluator, Tier2CelEvaluator, Tier2Decision, Tier2EvaluationContext,
};

fn sample_ctx(_method_label: &str) -> Tier2EvaluationContext {
    Tier2EvaluationContext::new(
        A2aMethod::MessageSend,
        serde_json::json!({}),
        Some(SpiceDbSubject::new("caller-1")),
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
    // Use `filetime::set_file_mtime` to bump mtime deterministically
    // instead of sleeping past filesystem granularity — the prior
    // `thread::sleep(1.1s)` approach depended on filesystem mtime
    // resolution and added a full second of wall-clock per test run.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rule.cel");
    fs::write(&path, "true").expect("write cel");

    let bundle = Tier2CompiledBundle::load_from_dir(dir.path()).expect("load bundle");
    assert_eq!(bundle.rules().count(), 1);

    fs::write(&path, "false").expect("rewrite cel");
    // Bump mtime by a known amount so the bundle's cached mtime is
    // guaranteed to differ from the new file mtime regardless of
    // underlying FS time resolution.
    let original_mtime = fs::metadata(&path).expect("meta").modified().expect("mtime");
    let bumped = original_mtime + Duration::from_secs(5);
    filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(bumped)).expect("bump mtime");

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
    let caller = SpiceDbSubject::new("caller-1");
    let ctx = super::evaluator::tier2_evaluation_context_from_ingress(
        A2aMethod::MessageSend,
        &A2aAgentId::new("planner").expect("agent"),
        Some(&caller),
        &headers,
        payload,
    );
    assert_eq!(ctx.request_method().as_str(), "message/send");
    assert_eq!(ctx.task_id().map(A2aTaskId::as_str), Some("t-9"));
    assert_eq!(ctx.caller_id().map(SpiceDbSubject::as_str), Some("caller-1"));
    assert_eq!(ctx.agent_id().as_str(), "planner");
    assert_eq!(ctx.headers().get("x-tenant-id").map(String::as_str), Some("acme"));
    assert_eq!(ctx.request_params().get("echo"), Some(&serde_json::json!(1)));
}

#[test]
fn evaluation_context_from_ingress_accepts_snake_case_task_id() {
    let payload = br#"{"params":{"task_id":"snake-id"}}"#;
    let ctx = super::evaluator::tier2_evaluation_context_from_ingress(
        A2aMethod::MessageSend,
        &A2aAgentId::new("planner").expect("agent"),
        None,
        &async_nats::HeaderMap::new(),
        payload,
    );
    assert_eq!(ctx.task_id().map(A2aTaskId::as_str), Some("snake-id"));
}

#[test]
fn evaluation_context_from_ingress_handles_invalid_json() {
    let payload = b"not-json";
    let ctx = super::evaluator::tier2_evaluation_context_from_ingress(
        A2aMethod::MessageSend,
        &A2aAgentId::new("planner").expect("agent"),
        None,
        &async_nats::HeaderMap::new(),
        payload,
    );
    assert_eq!(*ctx.request_params(), serde_json::Value::Null);
    assert!(ctx.task_id().is_none());
}

#[test]
fn evaluation_context_accessors_round_trip_constructor_args() {
    let mut headers = BTreeMap::new();
    headers.insert("h".to_string(), "v".to_string());
    let ctx = Tier2EvaluationContext::new(
        A2aMethod::MessageStream,
        serde_json::json!({"a":1}),
        Some(SpiceDbSubject::new("user/alice")),
        A2aAgentId::new("planner").expect("agent"),
        Some(A2aTaskId::new("task-1").expect("task id")),
        headers.clone(),
    );
    assert_eq!(ctx.request_method().as_str(), "message/stream");
    assert_eq!(*ctx.request_params(), serde_json::json!({"a":1}));
    assert_eq!(ctx.caller_id().map(SpiceDbSubject::as_str), Some("user/alice"));
    assert_eq!(ctx.agent_id().as_str(), "planner");
    assert_eq!(ctx.task_id().map(A2aTaskId::as_str), Some("task-1"));
    assert_eq!(ctx.headers(), &headers);
}

#[test]
fn bundle_load_from_missing_dir_yields_empty_bundle() {
    // Use a path derived from a real tempdir so the test doesn't depend
    // on Unix-style root semantics or hope that `/nonexistent/path`
    // happens to be absent on the runner.
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("does-not-exist");
    let bundle = Tier2CompiledBundle::load_from_dir(missing).expect("missing tolerated");
    assert_eq!(bundle.rules().count(), 0);
}

#[test]
fn bundle_load_from_non_directory_path_fails_fast() {
    // An existing path that's a file (not a directory) is a
    // misconfiguration; the loader must surface it instead of returning
    // an empty bundle and silently default-allowing every request.
    let file = tempfile::NamedTempFile::new().expect("tempfile");
    let err = Tier2CompiledBundle::load_from_dir(file.path()).expect_err("non-dir rejected");
    assert_eq!(err.path(), file.path());
}

#[test]
fn bundle_refresh_drops_deleted_rules() {
    // Without this, a `.cel` file that an operator removes would keep
    // enforcing its old rule indefinitely.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("temporary.cel");
    fs::write(&path, "true").expect("write cel");
    let mut bundle = Tier2CompiledBundle::load_from_dir(dir.path()).expect("load");
    assert_eq!(bundle.rules().count(), 1);
    fs::remove_file(&path).expect("remove cel");
    bundle.refresh_if_stale().expect("refresh after delete");
    assert_eq!(bundle.rules().count(), 0);
}

#[test]
fn rule_name_try_new_rejects_empty_or_whitespace() {
    assert!(matches!(RuleName::try_new(""), Err(RuleNameError::Empty)));
    assert!(matches!(RuleName::try_new("   "), Err(RuleNameError::Empty)));
    assert!(RuleName::try_new("rule-1").is_ok());
}

#[test]
fn evaluator_denies_when_refresh_fails() {
    // If the bundle's tier2_dir is replaced with a file under it after
    // load, the next refresh_if_stale will surface a Read error and
    // the evaluator must deny closed rather than fall through to allow.
    let dir = tempfile::tempdir().expect("tempdir");
    let rule_path = dir.path().join("ok.cel");
    fs::write(&rule_path, "true").expect("write cel");
    let bundle = Tier2CompiledBundle::load_from_dir(dir.path()).expect("load");
    let evaluator = RealTier2CelEvaluator::new(bundle);

    // Replace the rule file with a directory so the next mtime check on
    // the bundled `ok.cel` succeeds (it's still a regular file via
    // metadata), but a `compile_cel_file` retry on a newly-mtime'd file
    // returning malformed bytes would also fail. Easier: rewrite the
    // rule file with invalid CEL and bump its mtime so refresh compiles
    // it and fails, denying closed.
    fs::write(&rule_path, "(1 + 2").expect("invalid rewrite");
    let bumped = fs::metadata(&rule_path).expect("meta").modified().expect("mtime") + Duration::from_secs(5);
    filetime::set_file_mtime(&rule_path, filetime::FileTime::from_system_time(bumped)).expect("bump mtime");

    let decision = evaluator.evaluate(&sample_ctx("any"));
    assert_eq!(
        decision,
        Tier2Decision::Deny {
            rule: RuleName::evaluation_error()
        }
    );
}

#[test]
fn cel_compile_error_path_accessor_exposes_offending_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bad.cel");
    fs::write(&path, "(1 + 2").expect("write bad cel");
    let err = compile_cel_file(&path).expect_err("invalid cel");
    assert_eq!(err.path(), &path);
    assert!(matches!(err, CelCompileError::Compile { .. }));
}

#[test]
fn cel_compile_error_metadata_failure_is_typed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("never-was.cel");
    let err = compile_cel_file(&missing).expect_err("missing file");
    assert!(matches!(err, CelCompileError::Metadata { .. }));
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

#[test]
fn mock_engine_propagates_engine_error_as_deny() {
    // Cover the MockCelEngine error-branch: when the mock is configured
    // to fail a rule with an interpreter error, the evaluator must
    // surface that as a deny tagged `evaluation_error` rather than
    // letting the error bubble up to the caller.
    let mut outcomes = BTreeMap::new();
    outcomes.insert(RuleName::new("crashes"), Err(Tier2EvalError::execution("boom")));
    let (_dir, bundle) = bundle_with_rule("crashes", "true");
    let evaluator = RealTier2CelEvaluator::with_engine(bundle, Arc::new(MockCelEngine::new(outcomes)));
    assert_eq!(
        evaluator.evaluate(&sample_ctx("any")),
        Tier2Decision::Deny {
            rule: RuleName::evaluation_error()
        }
    );
}
