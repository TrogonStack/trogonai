use crate::{DriftSeverity, DriftType, ToolName};

/// A single drift event detected between two schema snapshots of the same
/// tool, or between two tool manifests. Matches AGT's `DriftAlert`
/// dataclass, with `details` replaced by a closed `DriftAlertDetails` enum
/// (AGT uses an untyped `dict[str, Any]`).
#[derive(Clone, Debug, PartialEq)]
pub struct DriftAlert {
    drift_type: DriftType,
    severity: DriftSeverity,
    tool_name: ToolName,
    message: String,
    details: DriftAlertDetails,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum DriftAlertDetails {
    #[default]
    None,
    DescriptionChanged {
        old: String,
        new: String,
    },
    ParameterAdded {
        parameter: String,
    },
    TypeChanged {
        parameter: String,
        old_type: String,
        new_type: String,
    },
    RequiredChanged {
        added_required: Vec<String>,
        removed_required: Vec<String>,
    },
}

impl DriftAlert {
    pub fn new(
        drift_type: DriftType,
        severity: DriftSeverity,
        tool_name: ToolName,
        message: impl Into<String>,
        details: DriftAlertDetails,
    ) -> Self {
        Self {
            drift_type,
            severity,
            tool_name,
            message: message.into(),
            details,
        }
    }

    pub fn drift_type(&self) -> DriftType {
        self.drift_type
    }

    pub fn severity(&self) -> DriftSeverity {
        self.severity
    }

    pub fn tool_name(&self) -> &ToolName {
        &self.tool_name
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn details(&self) -> &DriftAlertDetails {
        &self.details
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_all_constructed_fields() {
        let alert = DriftAlert::new(
            DriftType::DescriptionChanged,
            DriftSeverity::Info,
            ToolName::new("search").expect("valid"),
            "Tool 'search' description changed",
            DriftAlertDetails::DescriptionChanged {
                old: "old".to_string(),
                new: "new".to_string(),
            },
        );
        assert_eq!(alert.drift_type(), DriftType::DescriptionChanged);
        assert_eq!(alert.severity(), DriftSeverity::Info);
        assert_eq!(alert.tool_name().as_str(), "search");
        assert_eq!(alert.message(), "Tool 'search' description changed");
    }
}
