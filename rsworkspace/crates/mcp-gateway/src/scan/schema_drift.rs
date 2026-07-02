use crate::{
    DriftAlert, DriftAlertDetails, DriftReport, DriftSeverity, DriftType, ServerToolSnapshot, ToolSchemaSnapshot,
};

/// Compare a current tool manifest against a baseline and produce a
/// [`DriftReport`]. Pure function: the caller owns baseline storage and
/// lifetime (unlike AGT's `DriftDetector`, which keeps an internal
/// `_baselines` map and history list as part of the same object).
///
/// When `baseline` is `None` (no prior snapshot for this server), this
/// returns a report with no alerts and no baseline fingerprint, matching
/// AGT's `compare()` behavior of treating a missing baseline as "adopt this
/// as the baseline, no drift".
pub fn compare(baseline: Option<&ServerToolSnapshot>, current: &ServerToolSnapshot) -> DriftReport {
    let Some(baseline) = baseline else {
        return DriftReport::new(current.server_name().clone(), None, current.fingerprint(), Vec::new());
    };

    let mut alerts = Vec::new();

    for baseline_tool in baseline.tools() {
        if current.get_tool(baseline_tool.name()).is_none() {
            alerts.push(DriftAlert::new(
                DriftType::ToolRemoved,
                DriftSeverity::Critical,
                baseline_tool.name().clone(),
                format!(
                    "Tool '{}' was removed from server '{}'",
                    baseline_tool.name(),
                    current.server_name()
                ),
                DriftAlertDetails::None,
            ));
        }
    }

    for current_tool in current.tools() {
        if baseline.get_tool(current_tool.name()).is_none() {
            alerts.push(DriftAlert::new(
                DriftType::ToolAdded,
                DriftSeverity::Warning,
                current_tool.name().clone(),
                format!(
                    "New tool '{}' added to server '{}'",
                    current_tool.name(),
                    current.server_name()
                ),
                DriftAlertDetails::None,
            ));
        }
    }

    for current_tool in current.tools() {
        if let Some(baseline_tool) = baseline.get_tool(current_tool.name()) {
            alerts.extend(compare_tool(baseline_tool, current_tool));
        }
    }

    DriftReport::new(
        current.server_name().clone(),
        Some(baseline.fingerprint()),
        current.fingerprint(),
        alerts,
    )
}

/// Compare two versions of the same tool's schema. Matches AGT's
/// `DriftDetector._compare_tool` exactly: description change (INFO),
/// parameter added (CRITICAL if required else WARNING), parameter removed
/// (CRITICAL), type changed on a common parameter (CRITICAL), and required
/// set changed (CRITICAL if any parameter became non-required-that-was-required,
/// i.e. a required parameter was removed, else WARNING).
pub fn compare_tool(old: &ToolSchemaSnapshot, new: &ToolSchemaSnapshot) -> Vec<DriftAlert> {
    let mut alerts = Vec::new();
    let name = new.name();

    if old.description() != new.description() {
        alerts.push(DriftAlert::new(
            DriftType::DescriptionChanged,
            DriftSeverity::Info,
            name.clone(),
            format!("Tool '{name}' description changed"),
            DriftAlertDetails::DescriptionChanged {
                old: old.description().to_string(),
                new: new.description().to_string(),
            },
        ));
    }

    for parameter_name in new.parameters().keys() {
        if old.parameters().contains_key(parameter_name) {
            continue;
        }
        let is_required = new.is_required(parameter_name);
        let severity = if is_required {
            DriftSeverity::Critical
        } else {
            DriftSeverity::Warning
        };
        let suffix = if is_required { " (REQUIRED)" } else { "" };
        alerts.push(DriftAlert::new(
            DriftType::ParameterAdded,
            severity,
            name.clone(),
            format!("Parameter '{parameter_name}' added to tool '{name}'{suffix}"),
            DriftAlertDetails::ParameterAdded {
                parameter: parameter_name.clone(),
            },
        ));
    }

    for parameter_name in old.parameters().keys() {
        if new.parameters().contains_key(parameter_name) {
            continue;
        }
        alerts.push(DriftAlert::new(
            DriftType::ParameterRemoved,
            DriftSeverity::Critical,
            name.clone(),
            format!("Parameter '{parameter_name}' removed from tool '{name}'"),
            DriftAlertDetails::None,
        ));
    }

    for (parameter_name, new_schema) in new.parameters() {
        let Some(old_schema) = old.parameters().get(parameter_name) else {
            continue;
        };
        let old_type = old_schema.get("type").and_then(|v| v.as_str());
        let new_type = new_schema.get("type").and_then(|v| v.as_str());
        if let (Some(old_type), Some(new_type)) = (old_type, new_type)
            && old_type != new_type
        {
            alerts.push(DriftAlert::new(
                DriftType::TypeChanged,
                DriftSeverity::Critical,
                name.clone(),
                format!("Parameter '{parameter_name}' in tool '{name}' type changed: {old_type} -> {new_type}"),
                DriftAlertDetails::TypeChanged {
                    parameter: parameter_name.clone(),
                    old_type: old_type.to_string(),
                    new_type: new_type.to_string(),
                },
            ));
        }
    }

    let added_required: Vec<String> = new
        .required()
        .iter()
        .filter(|r| !old.required().contains(r))
        .cloned()
        .collect();
    let removed_required: Vec<String> = old
        .required()
        .iter()
        .filter(|r| !new.required().contains(r))
        .cloned()
        .collect();
    if !added_required.is_empty() || !removed_required.is_empty() {
        let severity = if removed_required.is_empty() {
            DriftSeverity::Warning
        } else {
            DriftSeverity::Critical
        };
        alerts.push(DriftAlert::new(
            DriftType::RequiredChanged,
            severity,
            name.clone(),
            format!("Required fields changed for tool '{name}'"),
            DriftAlertDetails::RequiredChanged {
                added_required,
                removed_required,
            },
        ));
    }

    alerts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ServerName, ToolName};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn tool(name: &str, description: &str, params: &[(&str, &str)], required: &[&str]) -> ToolSchemaSnapshot {
        let mut parameters = BTreeMap::new();
        for (param_name, param_type) in params {
            parameters.insert((*param_name).to_string(), json!({"type": param_type}));
        }
        ToolSchemaSnapshot::new(
            ToolName::new(name).expect("valid"),
            description,
            parameters,
            required.iter().map(|s| (*s).to_string()).collect(),
        )
    }

    fn snapshot(tools: Vec<ToolSchemaSnapshot>) -> ServerToolSnapshot {
        ServerToolSnapshot::new(ServerName::new("server-1").expect("valid"), tools, 1000.0)
    }

    #[test]
    fn no_baseline_yields_no_drift() {
        let current = snapshot(vec![tool("search", "d", &[], &[])]);
        let report = compare(None, &current);
        assert!(!report.has_drift());
        assert_eq!(report.baseline_fingerprint(), None);
    }

    #[test]
    fn tool_removed_is_critical() {
        let baseline = snapshot(vec![tool("search", "d", &[], &[])]);
        let current = snapshot(vec![]);
        let report = compare(Some(&baseline), &current);
        assert_eq!(report.alerts().len(), 1);
        assert_eq!(report.alerts()[0].drift_type(), DriftType::ToolRemoved);
        assert_eq!(report.alerts()[0].severity(), DriftSeverity::Critical);
    }

    #[test]
    fn tool_added_is_warning() {
        let baseline = snapshot(vec![]);
        let current = snapshot(vec![tool("search", "d", &[], &[])]);
        let report = compare(Some(&baseline), &current);
        assert_eq!(report.alerts().len(), 1);
        assert_eq!(report.alerts()[0].drift_type(), DriftType::ToolAdded);
        assert_eq!(report.alerts()[0].severity(), DriftSeverity::Warning);
    }

    #[test]
    fn description_changed_is_info() {
        let old = tool("search", "old description", &[], &[]);
        let new = tool("search", "new description", &[], &[]);
        let alerts = compare_tool(&old, &new);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].drift_type(), DriftType::DescriptionChanged);
        assert_eq!(alerts[0].severity(), DriftSeverity::Info);
    }

    #[test]
    fn optional_parameter_added_is_warning() {
        let old = tool("search", "d", &[], &[]);
        let new = tool("search", "d", &[("q", "string")], &[]);
        let alerts = compare_tool(&old, &new);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].drift_type(), DriftType::ParameterAdded);
        assert_eq!(alerts[0].severity(), DriftSeverity::Warning);
    }

    #[test]
    fn required_parameter_added_is_critical() {
        // Adding a required parameter fires both a critical ParameterAdded
        // alert and a RequiredChanged alert (the required set also changed);
        // AGT's own upstream test for this case asserts on a filtered count
        // for exactly this reason (see test_mcp_drift.py
        // test_required_parameter_added_is_critical).
        let old = tool("search", "d", &[], &[]);
        let new = tool("search", "d", &[("q", "string")], &["q"]);
        let alerts = compare_tool(&old, &new);
        let critical_param_added = alerts
            .iter()
            .filter(|a| a.drift_type() == DriftType::ParameterAdded && a.severity() == DriftSeverity::Critical)
            .count();
        assert_eq!(critical_param_added, 1);
    }

    #[test]
    fn parameter_removed_is_critical() {
        let old = tool("search", "d", &[("q", "string")], &[]);
        let new = tool("search", "d", &[], &[]);
        let alerts = compare_tool(&old, &new);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].drift_type(), DriftType::ParameterRemoved);
        assert_eq!(alerts[0].severity(), DriftSeverity::Critical);
    }

    #[test]
    fn type_changed_is_critical() {
        let old = tool("search", "d", &[("q", "string")], &[]);
        let new = tool("search", "d", &[("q", "integer")], &[]);
        let alerts = compare_tool(&old, &new);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].drift_type(), DriftType::TypeChanged);
        assert_eq!(alerts[0].severity(), DriftSeverity::Critical);
    }

    #[test]
    fn required_added_without_removal_is_warning() {
        let old = tool("search", "d", &[("q", "string"), ("r", "string")], &["q"]);
        let new = tool("search", "d", &[("q", "string"), ("r", "string")], &["q", "r"]);
        let alerts = compare_tool(&old, &new);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].drift_type(), DriftType::RequiredChanged);
        assert_eq!(alerts[0].severity(), DriftSeverity::Warning);
    }

    #[test]
    fn required_removed_is_critical() {
        let old = tool("search", "d", &[("q", "string")], &["q"]);
        let new = tool("search", "d", &[("q", "string")], &[]);
        let alerts = compare_tool(&old, &new);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].drift_type(), DriftType::RequiredChanged);
        assert_eq!(alerts[0].severity(), DriftSeverity::Critical);
    }

    #[test]
    fn multiple_changes_produce_multiple_alerts() {
        let old = tool("search", "old", &[("q", "string")], &["q"]);
        let new = tool("search", "new", &[("q", "integer"), ("r", "string")], &[]);
        let alerts = compare_tool(&old, &new);
        let types: Vec<DriftType> = alerts.iter().map(DriftAlert::drift_type).collect();
        assert!(types.contains(&DriftType::DescriptionChanged));
        assert!(types.contains(&DriftType::TypeChanged));
        assert!(types.contains(&DriftType::ParameterAdded));
        assert!(types.contains(&DriftType::RequiredChanged));
    }

    #[test]
    fn unchanged_tool_produces_no_alerts() {
        let old = tool("search", "d", &[("q", "string")], &["q"]);
        let new = tool("search", "d", &[("q", "string")], &["q"]);
        assert!(compare_tool(&old, &new).is_empty());
    }
}
