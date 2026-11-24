use std::num::NonZeroU32;

use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::IdleAction::{Pause, Release, Suspend};
use trogon_sandbox_contract::{IdleAction, IdlePolicy, IdlePolicyError, IdleStage};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn stages(stages: &[(u32, IdleAction)]) -> Result<Vec<IdleStage>, Box<dyn std::error::Error>> {
    stages
        .iter()
        .map(|&(after_seconds, action)| {
            Ok(IdleStage {
                after_seconds: NonZeroU32::try_from(after_seconds)?,
                action,
            })
        })
        .collect()
}

fn wire(stages: &[(u32, IdleAction)]) -> serde_json::Value {
    let stages: Vec<_> = stages
        .iter()
        .map(|&(after_seconds, action)| json!({ "after_seconds": after_seconds, "action": action }))
        .collect();
    json!({ "stages": stages })
}

#[test]
fn idle_policy_roundtrips_through_json() -> TestResult {
    let original = IdlePolicy::new(stages(&[(300, Pause), (3_600, Release)])?)?;
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
    assert_eq!(original.stages(), stages(&[(300, Pause), (3_600, Release)])?);
    Ok(())
}

#[test]
fn idle_actions_use_snake_case_names_ordered_by_severity() -> TestResult {
    for (action, name) in [(Pause, "pause"), (Suspend, "suspend"), (Release, "release")] {
        assert_eq!(serde_json::to_value(action)?, json!(name));
        assert_eq!(serde_json::from_value::<IdleAction>(json!(name))?, action);
    }
    assert!(serde_json::from_value::<IdleAction>(json!("stop")).is_err());
    assert!(Pause < Suspend);
    assert!(Suspend < Release);
    Ok(())
}

#[test]
fn idle_policy_accepts_escalating_stages() -> TestResult {
    for policy in [
        &[(60, Pause), (600, Release)][..],
        &[(60, Release)],
        &[(60, Pause), (600, Suspend), (3_600, Release)],
        &[(1, Pause), (u32::MAX, Release)],
    ] {
        let built = IdlePolicy::new(stages(policy)?)?;
        assert_eq!(serde_json::from_value::<IdlePolicy>(wire(policy))?, built);
    }
    Ok(())
}

#[test]
fn idle_policy_rejects_ambiguous_stages_on_construction_and_deserialization() -> TestResult {
    for (policy, expected) in [
        (&[][..], IdlePolicyError::NoStages),
        (&[(600, Pause), (60, Release)], IdlePolicyError::UnorderedStages),
        (&[(60, Pause), (60, Release)], IdlePolicyError::UnorderedStages),
        (&[(60, Release), (600, Pause)], IdlePolicyError::NonEscalatingActions),
        (&[(60, Pause), (600, Pause)], IdlePolicyError::NonEscalatingActions),
        (&[(60, Release), (600, Release)], IdlePolicyError::NonEscalatingActions),
    ] {
        assert_eq!(IdlePolicy::new(stages(policy)?).err(), Some(expected), "{policy:?}");
        let error = serde_json::from_value::<IdlePolicy>(wire(policy))
            .err()
            .ok_or("ambiguous idle policy deserialized")?;
        assert_eq!(error.classify(), Data, "{policy:?}");
    }
    Ok(())
}

#[test]
fn idle_policy_rejects_malformed_json() {
    for value in [
        wire(&[(0, Pause)]),
        json!({ "stages": [{ "after_seconds": -1, "action": "pause" }] }),
        json!({ "stages": [{ "after_seconds": 60 }] }),
        json!({}),
        json!([]),
    ] {
        assert!(serde_json::from_value::<IdlePolicy>(value.clone()).is_err(), "{value}");
    }
}

#[test]
fn idle_policy_fits_within_an_operator_ceiling() -> TestResult {
    let policy = IdlePolicy::new(stages(&[(60, Pause), (3_600, Release)])?)?;
    assert!(policy.fits_within(3_600));
    assert!(policy.fits_within(u32::MAX));
    assert!(!policy.fits_within(3_599));
    assert!(!policy.fits_within(0));
    Ok(())
}

#[test]
fn idle_policy_errors_describe_the_rule() {
    for (error, message) in [
        (IdlePolicyError::NoStages, "idle_policy needs at least one stage"),
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
