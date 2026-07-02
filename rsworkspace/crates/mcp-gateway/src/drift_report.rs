use crate::{DriftAlert, DriftSeverity, ServerName};

/// Complete drift analysis between a baseline and a current tool snapshot
/// for one server. Matches AGT's `DriftReport` dataclass.
#[derive(Clone, Debug, PartialEq)]
pub struct DriftReport {
    server_name: ServerName,
    baseline_fingerprint: Option<String>,
    current_fingerprint: String,
    alerts: Vec<DriftAlert>,
}

impl DriftReport {
    pub fn new(
        server_name: ServerName,
        baseline_fingerprint: Option<String>,
        current_fingerprint: String,
        alerts: Vec<DriftAlert>,
    ) -> Self {
        Self {
            server_name,
            baseline_fingerprint,
            current_fingerprint,
            alerts,
        }
    }

    pub fn server_name(&self) -> &ServerName {
        &self.server_name
    }

    pub fn baseline_fingerprint(&self) -> Option<&str> {
        self.baseline_fingerprint.as_deref()
    }

    pub fn current_fingerprint(&self) -> &str {
        &self.current_fingerprint
    }

    pub fn alerts(&self) -> &[DriftAlert] {
        &self.alerts
    }

    pub fn has_drift(&self) -> bool {
        !self.alerts.is_empty()
    }

    pub fn critical_count(&self) -> usize {
        self.alerts
            .iter()
            .filter(|a| a.severity() == DriftSeverity::Critical)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.alerts
            .iter()
            .filter(|a| a.severity() == DriftSeverity::Warning)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DriftAlertDetails, DriftType, ToolName};

    fn alert(severity: DriftSeverity) -> DriftAlert {
        DriftAlert::new(
            DriftType::ToolAdded,
            severity,
            ToolName::new("search").expect("valid"),
            "message",
            DriftAlertDetails::None,
        )
    }

    #[test]
    fn has_drift_reflects_alert_presence() {
        let empty = DriftReport::new(
            ServerName::new("s").expect("valid"),
            Some("abc".to_string()),
            "def".to_string(),
            Vec::new(),
        );
        assert!(!empty.has_drift());

        let with_alerts = DriftReport::new(
            ServerName::new("s").expect("valid"),
            Some("abc".to_string()),
            "def".to_string(),
            vec![alert(DriftSeverity::Info)],
        );
        assert!(with_alerts.has_drift());
    }

    #[test]
    fn counts_critical_and_warning_alerts_separately() {
        let report = DriftReport::new(
            ServerName::new("s").expect("valid"),
            None,
            "fp".to_string(),
            vec![
                alert(DriftSeverity::Critical),
                alert(DriftSeverity::Critical),
                alert(DriftSeverity::Warning),
                alert(DriftSeverity::Info),
            ],
        );
        assert_eq!(report.critical_count(), 2);
        assert_eq!(report.warning_count(), 1);
    }
}
