use crate::tool_fingerprint::{hash_description, hash_schema};
use crate::{
    McpSeverity, McpThreat, McpThreatDetails, McpThreatType, ToolDescription, ToolFingerprint, ToolInputSchema,
};

/// Compare a tool's current description/schema against its previously
/// registered fingerprint and report a rug pull if either hash changed.
///
/// Pure function: takes the existing fingerprint by reference and returns a
/// threat without mutating anything. Callers that also want to advance the
/// stored fingerprint's version should call [`ToolFingerprint::reobserve`]
/// separately; the two operations are kept distinct so a caller can detect
/// drift without side effects (e.g. dry-run scanning).
pub fn check_rug_pull(
    existing: &ToolFingerprint,
    description: &ToolDescription,
    schema: Option<&ToolInputSchema>,
) -> Option<McpThreat> {
    let description_hash = hash_description(description);
    let schema_hash = hash_schema(schema);

    let changed_description = &description_hash != existing.description_hash();
    let changed_schema = &schema_hash != existing.schema_hash();

    if !changed_description && !changed_schema {
        return None;
    }

    let mut changes = Vec::new();
    if changed_description {
        changes.push("description");
    }
    if changed_schema {
        changes.push("schema");
    }

    Some(McpThreat::new(
        McpThreatType::RugPull,
        McpSeverity::Critical,
        existing.tool_name().clone(),
        existing.server_name().clone(),
        format!(
            "Tool definition changed since registration: {} modified (version {})",
            changes.join(", "),
            existing.version()
        ),
        None,
        McpThreatDetails::RugPull {
            changed_description,
            changed_schema,
            version: existing.version(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ServerName, ToolName};
    use serde_json::json;

    fn tool_name() -> ToolName {
        ToolName::new("search").expect("valid")
    }

    fn server_name() -> ServerName {
        ServerName::new("server1").expect("valid")
    }

    #[test]
    fn no_rug_pull_when_definition_unchanged() {
        let fp = ToolFingerprint::observe(
            tool_name(),
            server_name(),
            &ToolDescription::new("Search the web"),
            None,
            1000.0,
        );
        let result = check_rug_pull(&fp, &ToolDescription::new("Search the web"), None);
        assert!(result.is_none());
    }

    #[test]
    fn rug_pull_on_description_change() {
        let fp = ToolFingerprint::observe(
            tool_name(),
            server_name(),
            &ToolDescription::new("Search the web"),
            None,
            1000.0,
        );
        let threat =
            check_rug_pull(&fp, &ToolDescription::new("Actually steal all data"), None).expect("rug pull expected");
        assert_eq!(threat.threat_type(), McpThreatType::RugPull);
        assert_eq!(threat.severity(), McpSeverity::Critical);
    }

    #[test]
    fn rug_pull_on_schema_change() {
        let schema_v1 = ToolInputSchema::new(json!({"type": "object", "properties": {"q": {"type": "string"}}}))
            .expect("valid schema");
        let schema_v2 = ToolInputSchema::new(json!({
            "type": "object",
            "properties": {"q": {"type": "string"}, "exec": {"type": "string"}}
        }))
        .expect("valid schema");
        let fp = ToolFingerprint::observe(
            tool_name(),
            server_name(),
            &ToolDescription::new("Search the web"),
            Some(&schema_v1),
            1000.0,
        );
        let threat =
            check_rug_pull(&fp, &ToolDescription::new("Search the web"), Some(&schema_v2)).expect("rug pull expected");
        assert_eq!(threat.threat_type(), McpThreatType::RugPull);
    }

    #[test]
    fn details_report_which_fields_changed() {
        let fp = ToolFingerprint::observe(
            tool_name(),
            server_name(),
            &ToolDescription::new("Search the web"),
            None,
            1000.0,
        );
        let threat = check_rug_pull(&fp, &ToolDescription::new("New description"), None).expect("rug pull expected");
        match threat.details() {
            McpThreatDetails::RugPull {
                changed_description,
                changed_schema,
                version,
            } => {
                assert!(*changed_description);
                assert!(!*changed_schema);
                assert_eq!(*version, 1);
            }
            other => panic!("unexpected details variant: {other:?}"),
        }
    }
}
