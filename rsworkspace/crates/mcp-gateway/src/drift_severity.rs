use std::fmt;

/// Severity of a single drift alert. Matches AGT's `DriftSeverity`:
/// `Info` for description changes and new optional parameters, `Warning`
/// for new tools and new required parameters, `Critical` for removed
/// tools, type changes, and removed required parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DriftSeverity {
    Info,
    Warning,
    Critical,
}

impl DriftSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

impl fmt::Display for DriftSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_info_below_warning_below_critical() {
        assert!(DriftSeverity::Info < DriftSeverity::Warning);
        assert!(DriftSeverity::Warning < DriftSeverity::Critical);
    }

    #[test]
    fn has_exactly_three_variants_with_stable_names() {
        assert_eq!(DriftSeverity::Info.as_str(), "info");
        assert_eq!(DriftSeverity::Warning.as_str(), "warning");
        assert_eq!(DriftSeverity::Critical.as_str(), "critical");
    }
}
