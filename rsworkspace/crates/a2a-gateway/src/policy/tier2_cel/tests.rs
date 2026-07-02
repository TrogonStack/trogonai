use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;
use std::time::Duration;

use a2a_auth_callout::SpiceDbSubject;
use a2a_nats::server::A2aMethod;
use a2a_nats::{A2aAgentId, A2aTaskId};
use chrono::{NaiveDate, TimeZone, Utc};

use super::bundle::Tier2CompiledBundle;
use super::compiler::{CelCompileError, compile_cel_file, compile_cel_source};
use super::evaluator::{CelEngine, CelInterpreterEngine, MockCelEngine, RealTier2CelEvaluator};
use crate::policy::RuleName;
use crate::policy::error::Tier2EvalError;
use crate::policy::tier2::resource_limits::Tier2ResourceLimits;
use crate::policy::tier2::rule_name::RuleNameError;
use crate::policy::tier2::{
    DenyAllTier2Evaluator, NoopTier2Evaluator, Tier2CelEvaluator, Tier2Decision, Tier2EvaluationContext,
};
use crate::policy::tier2_dynamic::{FixedTier2Clock, Tier2DynamicContext, WindowedBudget};

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
            rule: RuleName::new("deny_guests").expect("non-empty test rule name")
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
    outcomes.insert(
        RuleName::new("block_rule").expect("non-empty test rule name"),
        Ok(false),
    );
    let (_dir, bundle) = bundle_with_rule("block_rule", "true");
    let evaluator = RealTier2CelEvaluator::with_engine(bundle, Arc::new(MockCelEngine::new(outcomes)));
    assert_eq!(
        evaluator.evaluate(&sample_ctx("message/send")),
        Tier2Decision::Deny {
            rule: RuleName::new("block_rule").expect("non-empty test rule name")
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
    let name = RuleName::new("my-rule").expect("non-empty test rule name");
    assert_eq!(format!("{name}"), "my-rule");
    assert_eq!(name.as_str(), "my-rule");
    assert_eq!(RuleName::evaluation_error().as_str(), "evaluation_error");
}

#[test]
fn tier2_decision_is_allow_only_for_allow() {
    assert!(Tier2Decision::Allow.is_allow());
    assert!(
        !Tier2Decision::Deny {
            rule: RuleName::new("x").expect("non-empty test rule name"),
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
        &Tier2ResourceLimits::default(),
    )
    .expect("within limits");
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
        &Tier2ResourceLimits::default(),
    )
    .expect("within limits");
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
        &Tier2ResourceLimits::default(),
    )
    .expect("within limits");
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
fn rule_name_new_rejects_empty_or_whitespace() {
    // RuleName's public constructor is the sole validation point —
    // RuleName::new is fallible so an empty/whitespace name can't
    // produce a `RuleName` value. The unchecked constructor is
    // crate-private (`new_unchecked`) and only used by the loader on
    // pre-filtered file stems plus the `evaluation_error` sentinel.
    assert!(matches!(RuleName::new(""), Err(RuleNameError::Empty)));
    assert!(matches!(RuleName::new("   "), Err(RuleNameError::Empty)));
    assert!(RuleName::new("rule-1").is_ok());
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
    // silently treated as truthy/falsy. Match the typed variant
    // directly so a regression to `Execution` or `Binding` doesn't
    // slip past this test.
    let program = compile_cel_source("1 + 2").expect("compile");
    let engine = CelInterpreterEngine;
    let err = engine
        .evaluate_bool(
            &RuleName::new("non-bool").expect("non-empty test rule name"),
            &program,
            &sample_ctx("m"),
        )
        .expect_err("non-bool rejected");
    assert!(matches!(err, Tier2EvalError::NonBoolResult { .. }));
}

#[test]
fn mock_engine_propagates_engine_error_as_deny() {
    // Cover the MockCelEngine error-branch: when the mock is configured
    // to fail a rule with an interpreter error, the evaluator must
    // surface that as a deny tagged `evaluation_error` rather than
    // letting the error bubble up to the caller.
    let mut outcomes = BTreeMap::new();
    outcomes.insert(
        RuleName::new("crashes").expect("non-empty test rule name"),
        Err(Tier2EvalError::execution("boom")),
    );
    let (_dir, bundle) = bundle_with_rule("crashes", "true");
    let evaluator = RealTier2CelEvaluator::with_engine(bundle, Arc::new(MockCelEngine::new(outcomes)));
    assert_eq!(
        evaluator.evaluate(&sample_ctx("any")),
        Tier2Decision::Deny {
            rule: RuleName::evaluation_error()
        }
    );
}

#[test]
fn evaluator_denies_when_bundle_lock_poisoned() {
    // The bundle mutex is private to RealTier2CelEvaluator, so we lean
    // on a test-only hook to poison it deterministically. Without the
    // fail-closed branch, an unrelated panic somewhere in the runtime
    // would leave the policy layer wide open by silently allowing on
    // every subsequent evaluation.
    let (_dir, bundle) = bundle_with_rule("allow", "true");
    let evaluator = RealTier2CelEvaluator::new(bundle);
    evaluator.poison_bundle_for_test();
    assert_eq!(
        evaluator.evaluate(&sample_ctx("any")),
        Tier2Decision::Deny {
            rule: RuleName::evaluation_error()
        }
    );
}

#[test]
fn mock_engine_preserves_non_bool_result_variant() {
    let mut outcomes = BTreeMap::new();
    outcomes.insert(
        RuleName::new("non-bool-rule").expect("non-empty test rule name"),
        Err(Tier2EvalError::non_bool_result("Int(7)")),
    );
    let (_dir, bundle) = bundle_with_rule("non-bool-rule", "true");
    let evaluator = RealTier2CelEvaluator::with_engine(bundle, Arc::new(MockCelEngine::new(outcomes)));
    assert_eq!(
        evaluator.evaluate(&sample_ctx("any")),
        Tier2Decision::Deny {
            rule: RuleName::evaluation_error()
        }
    );
}

#[test]
fn mock_engine_preserves_binding_variant() {
    let mut outcomes = BTreeMap::new();
    outcomes.insert(
        RuleName::new("binding-fail").expect("non-empty test rule name"),
        Err(Tier2EvalError::binding("request", "ser failed")),
    );
    let (_dir, bundle) = bundle_with_rule("binding-fail", "true");
    let evaluator = RealTier2CelEvaluator::with_engine(bundle, Arc::new(MockCelEngine::new(outcomes)));
    assert_eq!(
        evaluator.evaluate(&sample_ctx("any")),
        Tier2Decision::Deny {
            rule: RuleName::evaluation_error()
        }
    );
}

#[test]
fn mock_engine_unspecified_rule_defaults_allow() {
    // When the mock has no entry for a rule, it defaults to Ok(true)
    // (allow). Without coverage on this branch, a regression to
    // default-deny would silently flip the gateway's open-by-default
    // semantics for unmapped rules.
    let outcomes = BTreeMap::new();
    let (_dir, bundle) = bundle_with_rule("unmapped", "true");
    let evaluator = RealTier2CelEvaluator::with_engine(bundle, Arc::new(MockCelEngine::new(outcomes)));
    assert_eq!(evaluator.evaluate(&sample_ctx("any")), Tier2Decision::Allow);
}

#[test]
fn evaluation_context_from_ingress_at_headers_limit_passes() {
    // "acme" (4 bytes) + "x-tenant-id" (11 bytes) = 15 bytes exactly.
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("x-tenant-id", "acme");
    let limits = Tier2ResourceLimits::new(15, usize::MAX, 64);
    let ctx = super::evaluator::tier2_evaluation_context_from_ingress(
        A2aMethod::MessageSend,
        &A2aAgentId::new("planner").expect("agent"),
        None,
        &headers,
        b"{}",
        &limits,
    );
    assert!(ctx.is_ok());
}

#[test]
fn evaluation_context_from_ingress_over_headers_limit_denies() {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("x-tenant-id", "acme");
    let limits = Tier2ResourceLimits::new(14, usize::MAX, 64);
    let err = super::evaluator::tier2_evaluation_context_from_ingress(
        A2aMethod::MessageSend,
        &A2aAgentId::new("planner").expect("agent"),
        None,
        &headers,
        b"{}",
        &limits,
    )
    .expect_err("headers over limit rejected");
    assert!(matches!(err, Tier2EvalError::HeadersTooLarge { actual: 15, limit: 14 }));
}

#[test]
fn evaluation_context_from_ingress_at_params_limit_passes() {
    let payload = br#"{"params":{"a":1}}"#;
    let params_size = super::evaluator::tier2_evaluation_context_from_ingress(
        A2aMethod::MessageSend,
        &A2aAgentId::new("planner").expect("agent"),
        None,
        &async_nats::HeaderMap::new(),
        payload,
        &Tier2ResourceLimits::default(),
    )
    .map(|ctx| serde_json::to_vec(ctx.request_params()).expect("serialize").len())
    .expect("within default limits");

    let limits = Tier2ResourceLimits::new(usize::MAX, params_size, 64);
    let ctx = super::evaluator::tier2_evaluation_context_from_ingress(
        A2aMethod::MessageSend,
        &A2aAgentId::new("planner").expect("agent"),
        None,
        &async_nats::HeaderMap::new(),
        payload,
        &limits,
    );
    assert!(ctx.is_ok());
}

#[test]
fn evaluation_context_from_ingress_over_params_limit_denies() {
    let payload = br#"{"params":{"a":1}}"#;
    let limits = Tier2ResourceLimits::new(usize::MAX, 1, 64);
    let err = super::evaluator::tier2_evaluation_context_from_ingress(
        A2aMethod::MessageSend,
        &A2aAgentId::new("planner").expect("agent"),
        None,
        &async_nats::HeaderMap::new(),
        payload,
        &limits,
    )
    .expect_err("params over limit rejected");
    assert!(matches!(err, Tier2EvalError::ParamsTooLarge { limit: 1, .. }));
}

#[test]
fn evaluation_context_from_ingress_deep_nesting_denies() {
    let mut nested = serde_json::json!(1);
    for _ in 0..100 {
        nested = serde_json::Value::Array(vec![nested]);
    }
    let payload = serde_json::to_vec(&serde_json::json!({ "params": { "a": nested } })).expect("serialize payload");
    let err = super::evaluator::tier2_evaluation_context_from_ingress(
        A2aMethod::MessageSend,
        &A2aAgentId::new("planner").expect("agent"),
        None,
        &async_nats::HeaderMap::new(),
        &payload,
        &Tier2ResourceLimits::default(),
    )
    .expect_err("deep nesting rejected");
    assert!(matches!(err, Tier2EvalError::NestingTooDeep { limit: 64 }));
}

#[test]
fn evaluation_context_from_ingress_at_max_nesting_depth_passes() {
    // `params.a` nested `max_depth` levels deep as arrays: the params
    // object itself is depth 1, so `max_depth - 1` further array levels
    // land the innermost array exactly at `max_depth`.
    let max_depth = 4;
    let mut nested = serde_json::json!(1);
    for _ in 0..(max_depth - 1) {
        nested = serde_json::Value::Array(vec![nested]);
    }
    let payload = serde_json::to_vec(&serde_json::json!({ "params": { "a": nested } })).expect("serialize payload");
    let limits = Tier2ResourceLimits::new(usize::MAX, usize::MAX, max_depth);
    let ctx = super::evaluator::tier2_evaluation_context_from_ingress(
        A2aMethod::MessageSend,
        &A2aAgentId::new("planner").expect("agent"),
        None,
        &async_nats::HeaderMap::new(),
        &payload,
        &limits,
    );
    assert!(ctx.is_ok());
}

#[test]
fn resource_limit_exceeded_rule_name_is_distinct_from_evaluation_error() {
    assert_ne!(RuleName::resource_limit_exceeded(), RuleName::evaluation_error());
    assert_eq!(RuleName::resource_limit_exceeded().as_str(), "resource_limit_exceeded");
}

fn bundle_with_rule_and_sidecar(
    name: &str,
    cel_source: &str,
    sidecar_toml: &str,
) -> (tempfile::TempDir, Tier2CompiledBundle) {
    let dir = tempfile::tempdir().expect("tempdir");
    let cel_path = dir.path().join(format!("{name}.cel"));
    std::fs::write(&cel_path, cel_source).expect("write cel");
    let sidecar_path = dir.path().join(format!("{name}.dynamic.toml"));
    std::fs::write(&sidecar_path, sidecar_toml).expect("write sidecar");
    let bundle = Tier2CompiledBundle::load_from_dir(dir.path()).expect("load");
    (dir, bundle)
}

fn fixed_clock_at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Arc<FixedTier2Clock> {
    let instant = Utc.from_utc_datetime(
        &NaiveDate::from_ymd_opt(y, mo, d)
            .and_then(|date| date.and_hms_opt(h, mi, 0))
            .expect("valid test datetime"),
    );
    Arc::new(FixedTier2Clock::new(instant))
}

#[test]
fn bundle_load_from_dir_loads_dynamic_condition_sidecar() {
    let (_dir, bundle) = bundle_with_rule_and_sidecar(
        "business_hours",
        "true",
        "type = \"time_window\"\nstart_time = \"09:00\"\nend_time = \"17:00\"\n",
    );
    assert_eq!(bundle.rules().count(), 1);
}

#[test]
fn bundle_load_from_dir_fails_closed_on_malformed_sidecar() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("bad_sidecar.cel"), "true").expect("write cel");
    std::fs::write(dir.path().join("bad_sidecar.dynamic.toml"), "type = \"day_of_week\"\n").expect("write sidecar");
    let err = Tier2CompiledBundle::load_from_dir(dir.path()).expect_err("missing days_of_week rejected");
    assert!(matches!(
        err,
        super::bundle_load_error::Tier2BundleLoadError::DynamicSidecar(_)
    ));
}

#[test]
fn evaluator_allows_when_cel_true_and_time_window_matches() {
    let (_dir, bundle) = bundle_with_rule_and_sidecar(
        "business_hours",
        "true",
        "type = \"time_window\"\nstart_time = \"09:00\"\nend_time = \"17:00\"\n",
    );
    let clock = fixed_clock_at(2026, 7, 2, 10, 0);
    let evaluator = RealTier2CelEvaluator::with_engine_and_dynamic_deps(
        bundle,
        Arc::new(CelInterpreterEngine),
        clock,
        WindowedBudget::new(),
    );
    assert_eq!(evaluator.evaluate(&sample_ctx("any")), Tier2Decision::Allow);
}

#[test]
fn evaluator_denies_with_distinct_rule_when_time_window_does_not_match() {
    let (_dir, bundle) = bundle_with_rule_and_sidecar(
        "business_hours",
        "true",
        "type = \"time_window\"\nstart_time = \"09:00\"\nend_time = \"17:00\"\n",
    );
    // 22:00 UTC is outside the 09:00-17:00 window.
    let clock = fixed_clock_at(2026, 7, 2, 22, 0);
    let evaluator = RealTier2CelEvaluator::with_engine_and_dynamic_deps(
        bundle,
        Arc::new(CelInterpreterEngine),
        clock,
        WindowedBudget::new(),
    );
    let decision = evaluator.evaluate(&sample_ctx("any"));
    assert_eq!(
        decision,
        Tier2Decision::Deny {
            rule: RuleName::new("business_hours.dynamic_condition").expect("non-empty test rule name")
        }
    );
    // Distinct from both a plain CEL-false deny and the generic evaluation_error sentinel.
    assert_ne!(
        decision,
        Tier2Decision::Deny {
            rule: RuleName::new("business_hours").expect("non-empty test rule name")
        }
    );
    assert_ne!(
        decision,
        Tier2Decision::Deny {
            rule: RuleName::evaluation_error()
        }
    );
}

#[test]
fn evaluator_dynamic_condition_not_checked_when_cel_already_false() {
    // If CEL evaluates false, the dynamic condition must NOT flip the
    // decision back to allow or change the reported rule -- AND
    // semantics only add restrictions, they never relax the base CEL
    // predicate's deny.
    let (_dir, bundle) = bundle_with_rule_and_sidecar(
        "always_false",
        "false",
        "type = \"time_window\"\nstart_time = \"00:00\"\nend_time = \"23:59\"\n",
    );
    let clock = fixed_clock_at(2026, 7, 2, 10, 0);
    let evaluator = RealTier2CelEvaluator::with_engine_and_dynamic_deps(
        bundle,
        Arc::new(CelInterpreterEngine),
        clock,
        WindowedBudget::new(),
    );
    assert_eq!(
        evaluator.evaluate(&sample_ctx("any")),
        Tier2Decision::Deny {
            rule: RuleName::new("always_false").expect("non-empty test rule name")
        }
    );
}

#[test]
fn evaluator_day_of_week_dynamic_condition_allows_matching_day() {
    let (_dir, bundle) = bundle_with_rule_and_sidecar(
        "weekday_only",
        "true",
        "type = \"day_of_week\"\ndays_of_week = [1, 2, 3, 4, 5]\n",
    );
    // 2026-07-02 is a Thursday.
    let clock = fixed_clock_at(2026, 7, 2, 10, 0);
    let evaluator = RealTier2CelEvaluator::with_engine_and_dynamic_deps(
        bundle,
        Arc::new(CelInterpreterEngine),
        clock,
        WindowedBudget::new(),
    );
    assert_eq!(evaluator.evaluate(&sample_ctx("any")), Tier2Decision::Allow);
}

#[test]
fn evaluator_day_of_week_dynamic_condition_denies_non_matching_day() {
    let (_dir, bundle) = bundle_with_rule_and_sidecar(
        "weekday_only",
        "true",
        "type = \"day_of_week\"\ndays_of_week = [1, 2, 3, 4, 5]\n",
    );
    // 2026-07-04 is a Saturday.
    let clock = fixed_clock_at(2026, 7, 4, 10, 0);
    let evaluator = RealTier2CelEvaluator::with_engine_and_dynamic_deps(
        bundle,
        Arc::new(CelInterpreterEngine),
        clock,
        WindowedBudget::new(),
    );
    assert_eq!(
        evaluator.evaluate(&sample_ctx("any")),
        Tier2Decision::Deny {
            rule: RuleName::new("weekday_only.dynamic_condition").expect("non-empty test rule name")
        }
    );
}

#[test]
fn evaluator_token_budget_allows_then_denies_across_calls() {
    let (_dir, bundle) = bundle_with_rule_and_sidecar(
        "token_capped",
        "true",
        "type = \"token_count_per_window\"\nwindow = \"1h\"\nlimit = 100\n",
    );
    let clock = fixed_clock_at(2026, 7, 2, 10, 0);
    let evaluator = RealTier2CelEvaluator::with_engine_and_dynamic_deps(
        bundle,
        Arc::new(CelInterpreterEngine),
        clock,
        WindowedBudget::new(),
    );

    let ctx_60 = sample_ctx("any").with_dynamic_context(Tier2DynamicContext::empty().with_budget_token_count(60.0));
    assert_eq!(evaluator.evaluate(&ctx_60), Tier2Decision::Allow);

    // Second call consumes another 60 tokens; cumulative 120 > limit 100.
    let decision = evaluator.evaluate(&ctx_60);
    assert_eq!(
        decision,
        Tier2Decision::Deny {
            rule: RuleName::new("token_capped.dynamic_condition").expect("non-empty test rule name")
        }
    );
}

#[test]
fn evaluator_cost_budget_allows_at_exact_limit() {
    let (_dir, bundle) = bundle_with_rule_and_sidecar(
        "cost_capped",
        "true",
        "type = \"cost_per_window\"\nwindow = \"1h\"\nlimit = 10\n",
    );
    let clock = fixed_clock_at(2026, 7, 2, 10, 0);
    let evaluator = RealTier2CelEvaluator::with_engine_and_dynamic_deps(
        bundle,
        Arc::new(CelInterpreterEngine),
        clock,
        WindowedBudget::new(),
    );
    let ctx = sample_ctx("any").with_dynamic_context(Tier2DynamicContext::empty().with_budget_cost(10.0));
    assert_eq!(evaluator.evaluate(&ctx), Tier2Decision::Allow);
}

#[test]
fn evaluator_without_sidecar_ignores_dynamic_condition_path_entirely() {
    // A rule with no `.dynamic.toml` sidecar must behave exactly as
    // before WI-09 -- CEL alone drives the decision.
    let (_dir, bundle) = bundle_with_rule("no_sidecar", "true");
    let clock = fixed_clock_at(2026, 7, 2, 10, 0);
    let evaluator = RealTier2CelEvaluator::with_engine_and_dynamic_deps(
        bundle,
        Arc::new(CelInterpreterEngine),
        clock,
        WindowedBudget::new(),
    );
    assert_eq!(evaluator.evaluate(&sample_ctx("any")), Tier2Decision::Allow);
}
