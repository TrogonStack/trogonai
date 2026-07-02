use crate::VulnerabilityRecord;

/// Outcome of a pre-dispatch CVE feed check for a registered MCP server
/// package. There is no silent-allow state: every non-`Allow` path,
/// including a feed that could not be reached, is a variant of `Deny`.
#[derive(Clone, Debug, PartialEq)]
pub enum FeedVerdict {
    /// The feed was reachable and reported no known vulnerabilities for
    /// this package/version.
    Allow,
    /// The feed reported one or more known vulnerabilities.
    DenyVulnerable(Vec<VulnerabilityRecord>),
    /// The feed could not be reached or returned a response the client
    /// could not parse. Fail-closed by default: unknown is treated as
    /// denied unless the caller opts into `FailOpenPolicy::Open`.
    DenyUnknown(DenyUnknownReason),
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DenyUnknownReason {
    #[error("OSV feed unreachable: {0}")]
    FeedUnreachable(String),
    #[error("OSV feed returned malformed response: {0}")]
    MalformedResponse(String),
}

impl FeedVerdict {
    /// True only for the explicit clean-scan result. Every other variant,
    /// including `DenyUnknown`, must block dispatch.
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }

    pub fn vulnerabilities(&self) -> &[VulnerabilityRecord] {
        match self {
            Self::DenyVulnerable(records) => records,
            Self::Allow | Self::DenyUnknown(_) => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CveId, VulnerabilitySeverity};

    #[test]
    fn allow_is_the_only_allowed_variant() {
        assert!(FeedVerdict::Allow.is_allowed());
        assert!(!FeedVerdict::DenyVulnerable(Vec::new()).is_allowed());
        assert!(!FeedVerdict::DenyUnknown(DenyUnknownReason::FeedUnreachable("timeout".into())).is_allowed());
    }

    #[test]
    fn deny_vulnerable_exposes_records() {
        let record = VulnerabilityRecord::new(
            CveId::new("CVE-2024-1").expect("valid"),
            VulnerabilitySeverity::High,
            "issue",
            None,
            Vec::new(),
        );
        let verdict = FeedVerdict::DenyVulnerable(vec![record.clone()]);
        assert_eq!(verdict.vulnerabilities(), [record]);
    }

    #[test]
    fn allow_and_deny_unknown_expose_no_vulnerabilities() {
        assert!(FeedVerdict::Allow.vulnerabilities().is_empty());
        assert!(
            FeedVerdict::DenyUnknown(DenyUnknownReason::MalformedResponse("bad json".into()))
                .vulnerabilities()
                .is_empty()
        );
    }
}
