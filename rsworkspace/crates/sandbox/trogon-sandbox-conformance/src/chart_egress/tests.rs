use std::error::Error;
use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;
use trogon_sandbox_contract::EgressEnforcementLevel;

use super::{chart_egress_enforcement, ensure_chart_egress_at_least_profile, ensure_chart_egress_matches_profile};
use crate::ChartEgressError;

type Fallible<T = ()> = Result<T, Box<dyn Error>>;

/// A crate directory four levels below a repository root whose `worker` chart has `values` as its values file.
fn layout(values: &str) -> Fallible<(TempDir, PathBuf)> {
    let root = tempfile::tempdir()?;
    let manifest_dir = root.path().join("rsworkspace/crates/sandbox/worker");
    fs::create_dir_all(&manifest_dir)?;
    let chart = root.path().join("devops/helm/charts/worker");
    fs::create_dir_all(&chart)?;
    fs::write(chart.join("values.yaml"), values)?;
    Ok((root, manifest_dir))
}

#[test]
fn a_chart_level_is_read_in_its_kebab_case_spelling() -> Fallible {
    for (values, level) in [
        ("egressEnforcement: none\n", EgressEnforcementLevel::None),
        (
            "replicas: 1\negressEnforcement: best-effort\n",
            EgressEnforcementLevel::BestEffort,
        ),
        ("egressEnforcement: \"mandatory\"\n", EgressEnforcementLevel::Mandatory),
    ] {
        let (_root, dir) = layout(values)?;
        assert_eq!(chart_egress_enforcement(&dir, "worker")?, level, "{values}");
    }
    Ok(())
}

#[test]
fn a_chart_without_the_value_is_missing() -> Fallible {
    let (_root, dir) = layout("replicas: 1\n")?;
    let error = chart_egress_enforcement(&dir, "worker");
    assert!(matches!(error, Err(ChartEgressError::Missing { .. })), "{error:?}");
    Ok(())
}

#[test]
fn an_unknown_level_is_refused() -> Fallible {
    let (_root, dir) = layout("egressEnforcement: strict\n")?;
    let error = chart_egress_enforcement(&dir, "worker");
    assert!(
        matches!(&error, Err(ChartEgressError::Unknown { raw, .. }) if raw == "strict"),
        "{error:?}"
    );
    Ok(())
}

#[test]
fn a_missing_chart_is_a_read_error() -> Fallible {
    let (_root, dir) = layout("egressEnforcement: none\n")?;
    let error = chart_egress_enforcement(&dir, "absent");
    assert!(matches!(error, Err(ChartEgressError::Read { .. })), "{error:?}");
    Ok(())
}

#[test]
fn a_matching_chart_must_equal_the_profile() -> Fallible {
    let (_root, dir) = layout("egressEnforcement: best-effort\n")?;
    ensure_chart_egress_matches_profile(&dir, "worker", EgressEnforcementLevel::BestEffort)?;
    for profile in [EgressEnforcementLevel::None, EgressEnforcementLevel::Mandatory] {
        let error = ensure_chart_egress_matches_profile(&dir, "worker", profile);
        assert!(matches!(error, Err(ChartEgressError::Drifted { .. })), "{error:?}");
    }
    Ok(())
}

#[test]
fn a_chart_may_be_stricter_than_the_profile_but_never_looser() -> Fallible {
    let (_root, dir) = layout("egressEnforcement: best-effort\n")?;
    ensure_chart_egress_at_least_profile(&dir, "worker", EgressEnforcementLevel::None)?;
    ensure_chart_egress_at_least_profile(&dir, "worker", EgressEnforcementLevel::BestEffort)?;
    let error = ensure_chart_egress_at_least_profile(&dir, "worker", EgressEnforcementLevel::Mandatory);
    assert!(matches!(error, Err(ChartEgressError::Weaker { .. })), "{error:?}");
    Ok(())
}
