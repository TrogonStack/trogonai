use std::fmt;

/// Category of MCP tool schema drift, matching AGT's `DriftType` enum in
/// `agent_sre.integrations.mcp` and MCP-SECURITY-GATEWAY-1.0 section 16.2
/// exactly: 8 variants.
///
/// `SchemaChanged` is defined for API completeness (parity with upstream and
/// the spec's enum) but, matching AGT's own `DriftDetector._compare_tool`,
/// is never emitted by [`crate::scan::schema_drift::compare`]: every schema
/// change AGT detects is already classified into one of the more specific
/// variants below (parameter added/removed, type changed, required changed,
/// description changed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DriftType {
    ToolAdded,
    ToolRemoved,
    SchemaChanged,
    ParameterAdded,
    ParameterRemoved,
    TypeChanged,
    DescriptionChanged,
    RequiredChanged,
}

impl DriftType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolAdded => "tool_added",
            Self::ToolRemoved => "tool_removed",
            Self::SchemaChanged => "schema_changed",
            Self::ParameterAdded => "parameter_added",
            Self::ParameterRemoved => "parameter_removed",
            Self::TypeChanged => "type_changed",
            Self::DescriptionChanged => "description_changed",
            Self::RequiredChanged => "required_changed",
        }
    }
}

impl fmt::Display for DriftType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_exactly_eight_variants_with_stable_names() {
        let names = [
            DriftType::ToolAdded.as_str(),
            DriftType::ToolRemoved.as_str(),
            DriftType::SchemaChanged.as_str(),
            DriftType::ParameterAdded.as_str(),
            DriftType::ParameterRemoved.as_str(),
            DriftType::TypeChanged.as_str(),
            DriftType::DescriptionChanged.as_str(),
            DriftType::RequiredChanged.as_str(),
        ];
        assert_eq!(
            names,
            [
                "tool_added",
                "tool_removed",
                "schema_changed",
                "parameter_added",
                "parameter_removed",
                "type_changed",
                "description_changed",
                "required_changed",
            ]
        );
    }
}
