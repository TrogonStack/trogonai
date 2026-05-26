#[cfg(test)]
mod compiler_tests {
    use std::fs;
    use std::thread;
    use std::time::Duration;

    use super::super::bundle::Tier2CompiledBundle;
    use super::super::compiler::{compile_cel_file, compile_cel_source};

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
        let mut ctx = cel_interpreter::Context::default();
        let (_, program) = refreshed.rules().next().expect("rule");
        let value = program
            .program()
            .execute(&ctx)
            .expect("execute refreshed program");
        assert_eq!(value, cel_interpreter::Value::Bool(false));

        let (_, program_before) = compile_cel_file(&path).expect("compile file");
        let _ = program_before;
        let _ = &mut ctx;
    }
}

#[cfg(test)]
mod evaluator_tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use a2a_nats::A2aAgentId;

    use crate::policy::tier2::{Tier2CelEvaluator, Tier2Decision, Tier2EvaluationContext};
    use crate::policy::RuleName;
    use crate::policy::tier2_cel::bundle::Tier2CompiledBundle;
    use crate::policy::tier2_cel::compiler::compile_cel_source;
    use crate::policy::tier2_cel::evaluator::{MockCelEngine, RealTier2CelEvaluator};

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
    fn evaluator_allow_path() {
        let (_dir, bundle) = bundle_with_rule("allow_all", "true");
        let evaluator = RealTier2CelEvaluator::new(bundle);
        assert_eq!(
            evaluator.evaluate(&sample_ctx("message/send")),
            Tier2Decision::Allow
        );
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
        let evaluator = RealTier2CelEvaluator::with_engine(
            bundle,
            Arc::new(MockCelEngine::new(outcomes)),
        );
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
}
