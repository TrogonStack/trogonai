//! Keeps a worker's Helm chart from drifting away from its capability profile.
//!
//! The chart's `egressEnforcement` value labels the worker pod, and that label selects the NetworkPolicy around it.
//! A worker test passes its own `CARGO_MANIFEST_DIR` and chart name, so a profile change that the chart does not
//! follow fails the worker's own build.

use std::path::Path;

use trogon_sandbox_contract::EgressEnforcementLevel;

use crate::ChartEgressError;
use crate::constants::SANDBOX_CHARTS;

/// The `egressEnforcement` value `chart` renders onto the worker pod.
///
/// `manifest_dir` is the calling worker crate's `CARGO_MANIFEST_DIR`.
pub fn chart_egress_enforcement(
    manifest_dir: impl AsRef<Path>,
    chart: &str,
) -> Result<EgressEnforcementLevel, ChartEgressError> {
    let values = manifest_dir
        .as_ref()
        .join(SANDBOX_CHARTS)
        .join(chart)
        .join("values.yaml");
    let text = std::fs::read_to_string(&values).map_err(|source| ChartEgressError::Read {
        path: values.display().to_string(),
        source,
    })?;
    let raw = text
        .lines()
        .find_map(|line| line.strip_prefix("egressEnforcement:"))
        .ok_or_else(|| ChartEgressError::Missing {
            chart: chart.to_owned(),
        })?
        .trim()
        .trim_matches(|c| c == '"' || c == '\'');
    serde_json::from_value(serde_json::Value::String(raw.replace('-', "_"))).map_err(|source| {
        ChartEgressError::Unknown {
            chart: chart.to_owned(),
            raw: raw.to_owned(),
            source,
        }
    })
}

/// `Ok` when `chart` renders exactly the profile's level.
pub fn ensure_chart_egress_matches_profile(
    manifest_dir: impl AsRef<Path>,
    chart: &str,
    profile_level: EgressEnforcementLevel,
) -> Result<(), ChartEgressError> {
    let chart_level = chart_egress_enforcement(manifest_dir, chart)?;
    if chart_level == profile_level {
        return Ok(());
    }
    Err(ChartEgressError::Drifted {
        chart: chart.to_owned(),
        chart_level,
        profile_level,
    })
}

/// `Ok` when `chart` renders the profile's level or a stricter one.
///
/// For a worker whose profile claims less than the pod around it enforces: the chart label selects the worker pod's
/// own NetworkPolicy, so it may stay stricter than the guest-facing claim, but never looser.
pub fn ensure_chart_egress_at_least_profile(
    manifest_dir: impl AsRef<Path>,
    chart: &str,
    profile_level: EgressEnforcementLevel,
) -> Result<(), ChartEgressError> {
    let chart_level = chart_egress_enforcement(manifest_dir, chart)?;
    if chart_level >= profile_level {
        return Ok(());
    }
    Err(ChartEgressError::Weaker {
        chart: chart.to_owned(),
        chart_level,
        profile_level,
    })
}

#[cfg(test)]
mod tests;
