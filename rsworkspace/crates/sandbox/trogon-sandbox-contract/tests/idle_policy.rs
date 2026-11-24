use serde_json::json;
use trogon_sandbox_contract::IdleAction::{Pause, Release, Suspend};
use trogon_sandbox_contract::{IdleAction, IdlePolicy, IdlePolicyError, IdleStage};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MAX_SECONDS: u32 = 86_400;

fn policy(stages: &[(u32, IdleAction)]) -> IdlePolicy {
    IdlePolicy {
        stages: stages
            .iter()
            .map(|&(after_seconds, action)| IdleStage { after_seconds, action })
            .collect(),
    }
}

#[test]
fn idle_policy_roundtrips_through_json() -> TestResult {
    let original = policy(&[(300, IdleAction::Pause), (3_600, IdleAction::Release)]);
    let wire = serde_json::to_value(&original)?;
    assert_eq!(
        wire,
        json!({
            "stages": [
                { "after_seconds": 300, "action": "pause" },
                { "after_seconds": 3600, "action": "release" },
            ]
        })
    );
    assert_eq!(serde_json::from_value::<IdlePolicy>(wire)?, original);
    Ok(())
}

#[test]
fn idle_actions_use_snake_case_names_ordered_by_severity() -> TestResult {
    for (action, name) in [
        (IdleAction::Pause, "pause"),
        (IdleAction::Suspend, "suspend"),
        (IdleAction::Release, "release"),
    ] {
        assert_eq!(serde_json::to_value(action)?, json!(name));
        assert_eq!(serde_json::from_value::<IdleAction>(json!(name))?, action);
    }
    assert!(serde_json::from_value::<IdleAction>(json!("stop")).is_err());
    assert!(IdleAction::Pause < IdleAction::Suspend);
    assert!(IdleAction::Suspend < IdleAction::Release);
    Ok(())
}

#[test]
fn idle_policy_validation_accepts_escalating_stages() -> TestResult {
    for stages in [
        &[(60, Pause), (600, Release)][..],
        &[(60, Release)],
        &[(60, Pause), (600, Suspend), (3_600, Release)],
        &[(1, Pause), (MAX_SECONDS, Release)],
    ] {
        policy(stages).validate(MAX_SECONDS)?;
    }
    Ok(())
}

#[test]
fn idle_policy_validation_rejects_ambiguous_policies() {
    let out_of_range = IdlePolicyError::StageOutOfRange {
        max_seconds: MAX_SECONDS,
    };
    for (stages, expected) in [
        (&[][..], IdlePolicyError::NoStages),
        (&[(0, Pause)], out_of_range),
        (&[(MAX_SECONDS + 1, Release)], out_of_range),
        (&[(600, Pause), (60, Release)], IdlePolicyError::UnorderedStages),
        (&[(60, Pause), (60, Release)], IdlePolicyError::UnorderedStages),
        (&[(60, Release), (600, Pause)], IdlePolicyError::NonEscalatingActions),
        (&[(60, Pause), (600, Pause)], IdlePolicyError::NonEscalatingActions),
        (&[(60, Release), (600, Release)], IdlePolicyError::NonEscalatingActions),
    ] {
        assert_eq!(policy(stages).validate(MAX_SECONDS).err(), Some(expected), "{stages:?}");
    }
}

#[test]
fn idle_policy_errors_describe_the_rule() {
    for (error, message) in [
        (IdlePolicyError::NoStages, "idle_policy needs at least one stage"),
        (
            IdlePolicyError::StageOutOfRange {
                max_seconds: MAX_SECONDS,
            },
            "idle_policy stage after_seconds must be between 1 and 86400",
        ),
        (
            IdlePolicyError::UnorderedStages,
            "idle_policy stages must be sorted by strictly increasing after_seconds",
        ),
        (
            IdlePolicyError::NonEscalatingActions,
            "idle_policy actions must get strictly more severe (pause, suspend, then release)",
        ),
    ] {
        assert_eq!(error.to_string(), message);
    }
}
