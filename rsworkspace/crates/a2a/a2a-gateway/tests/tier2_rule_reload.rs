use std::collections::BTreeMap;

use a2a_gateway::policy::{
    RealTier2CelEvaluator, RuleName, Tier2CelEvaluator, Tier2CompiledBundle, Tier2Decision, Tier2EvaluationContext,
};
use a2a_nats::{A2aAgentId, A2aMethod};

#[test]
fn same_mtime_rewrites_update_policy_and_keep_invalid_rules_denied() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("access.cel");
    std::fs::write(&path, "true  ")?;
    let original_mtime = std::fs::metadata(&path)?.modified()?;
    let evaluator = RealTier2CelEvaluator::new(Tier2CompiledBundle::load_from_dir(root.path())?);
    let context = Tier2EvaluationContext::new(
        A2aMethod::MessageSend,
        serde_json::json!({}),
        None,
        A2aAgentId::new("planner")?,
        None,
        BTreeMap::new(),
    );
    assert_eq!(evaluator.evaluate(&context), Tier2Decision::Allow);

    for (source, expected) in [
        (
            "false ",
            Tier2Decision::Deny {
                rule: RuleName::new("access")?,
            },
        ),
        (
            "(1 + 2",
            Tier2Decision::Deny {
                rule: RuleName::evaluation_error(),
            },
        ),
        ("true  ", Tier2Decision::Allow),
    ] {
        std::fs::write(&path, source)?;
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(original_mtime))?;
        assert_eq!(std::fs::metadata(&path)?.modified()?, original_mtime);
        assert_eq!(std::fs::metadata(&path)?.len(), 6);
        assert_eq!(evaluator.evaluate(&context), expected);
        assert_eq!(evaluator.evaluate(&context), expected);
    }
    Ok(())
}
