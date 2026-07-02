use serde::Deserialize;

use crate::{CveId, VulnerabilityRecord, VulnerabilitySeverity};

/// Response body from `POST https://api.osv.dev/v1/query`. Field shapes
/// mirror AGT's `_parse_osv_response`; every field is optional because OSV
/// omits absent data rather than sending nulls or empty containers.
#[derive(Debug, Deserialize)]
pub struct OsvResponseWire {
    #[serde(default)]
    vulns: Vec<OsvVulnWire>,
}

#[derive(Debug, Deserialize)]
struct OsvVulnWire {
    #[serde(default)]
    id: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    details: String,
    #[serde(default)]
    severity: Vec<OsvSeverityWire>,
    #[serde(default)]
    affected: Vec<OsvAffectedWire>,
    #[serde(default)]
    references: Vec<OsvReferenceWire>,
}

#[derive(Debug, Deserialize)]
struct OsvSeverityWire {
    #[serde(default)]
    score: String,
}

#[derive(Debug, Deserialize)]
struct OsvAffectedWire {
    #[serde(default)]
    ranges: Vec<OsvRangeWire>,
}

#[derive(Debug, Deserialize)]
struct OsvRangeWire {
    #[serde(default)]
    events: Vec<OsvEventWire>,
}

#[derive(Debug, Deserialize)]
struct OsvEventWire {
    fixed: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OsvReferenceWire {
    url: Option<String>,
}

/// Maximum references kept per finding, matching AGT's `refs[:5]` cap.
const MAX_REFERENCES: usize = 5;

impl OsvResponseWire {
    /// Parse the raw OSV.dev JSON body. The only failure mode is malformed
    /// JSON / an unexpected shape; a well-formed empty `vulns` array is a
    /// legitimate clean result, not an error.
    pub fn parse(body: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(body)
    }

    /// Convert into domain records, matching AGT's `_parse_osv_response`
    /// severity/CVE-alias/fixed-version extraction rules.
    pub fn into_vulnerability_records(self) -> Vec<VulnerabilityRecord> {
        self.vulns
            .into_iter()
            .map(OsvVulnWire::into_vulnerability_record)
            .collect()
    }
}

impl OsvVulnWire {
    fn into_vulnerability_record(self) -> VulnerabilityRecord {
        let severity = self
            .severity
            .iter()
            .filter_map(|s| parse_cvss_score(&s.score))
            .map(VulnerabilitySeverity::from_cvss_score)
            .max()
            .unwrap_or(VulnerabilitySeverity::Unknown);

        let cve_id_str = self
            .aliases
            .iter()
            .find(|alias| alias.starts_with("CVE-"))
            .cloned()
            .unwrap_or(self.id);
        let cve_id = CveId::new(cve_id_str).unwrap_or_else(|_| {
            // OSV always sends a non-empty `id`; this only triggers if both
            // `id` and every alias were empty strings, an OSV protocol
            // violation. Fall back to a sentinel rather than panicking so a
            // single malformed entry cannot take down the whole response.
            #[allow(clippy::unwrap_used)]
            CveId::new("UNKNOWN").unwrap()
        });

        let fixed_version = self
            .affected
            .iter()
            .flat_map(|affected| &affected.ranges)
            .flat_map(|range| &range.events)
            .find_map(|event| event.fixed.clone());

        let references = self
            .references
            .into_iter()
            .filter_map(|r| r.url)
            .take(MAX_REFERENCES)
            .collect();

        let summary = if !self.summary.is_empty() {
            self.summary
        } else {
            self.details.chars().take(200).collect()
        };

        VulnerabilityRecord::new(cve_id, severity, summary, fixed_version, references)
    }
}

/// OSV scores are either a bare number (`"9.8"`) or a fraction-style string
/// (`"9.8/10"`), matching AGT's `score.split("/")[0]` handling.
fn parse_cvss_score(score: &str) -> Option<f64> {
    if score.is_empty() {
        return None;
    }
    let head = score.split('/').next().unwrap_or(score);
    head.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_vulns_as_clean() {
        let body = br#"{"vulns": []}"#;
        let response = OsvResponseWire::parse(body).expect("valid json");
        assert!(response.into_vulnerability_records().is_empty());
    }

    #[test]
    fn parses_missing_vulns_field_as_clean() {
        let body = br#"{}"#;
        let response = OsvResponseWire::parse(body).expect("valid json");
        assert!(response.into_vulnerability_records().is_empty());
    }

    #[test]
    fn rejects_malformed_json() {
        let body = br#"{not json"#;
        assert!(OsvResponseWire::parse(body).is_err());
    }

    #[test]
    fn extracts_cve_alias_over_osv_id() {
        let body = br#"{
            "vulns": [
                {
                    "id": "GHSA-xxxx-yyyy-zzzz",
                    "aliases": ["CVE-2024-99999"],
                    "summary": "remote code execution",
                    "severity": [{"score": "9.8"}]
                }
            ]
        }"#;
        let records = OsvResponseWire::parse(body)
            .expect("valid json")
            .into_vulnerability_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].cve_id().as_str(), "CVE-2024-99999");
        assert_eq!(records[0].severity(), VulnerabilitySeverity::Critical);
        assert_eq!(records[0].summary(), "remote code execution");
    }

    #[test]
    fn falls_back_to_osv_id_when_no_cve_alias() {
        let body = br#"{
            "vulns": [
                {"id": "GHSA-xxxx-yyyy-zzzz", "summary": "issue"}
            ]
        }"#;
        let records = OsvResponseWire::parse(body)
            .expect("valid json")
            .into_vulnerability_records();
        assert_eq!(records[0].cve_id().as_str(), "GHSA-xxxx-yyyy-zzzz");
        assert_eq!(records[0].severity(), VulnerabilitySeverity::Unknown);
    }

    #[test]
    fn extracts_fixed_version_from_ranges() {
        let body = br#"{
            "vulns": [{
                "id": "GHSA-1",
                "affected": [{"ranges": [{"events": [{"introduced": "0"}, {"fixed": "1.2.1"}]}]}]
            }]
        }"#;
        let records = OsvResponseWire::parse(body)
            .expect("valid json")
            .into_vulnerability_records();
        assert_eq!(records[0].fixed_version(), Some("1.2.1"));
    }

    #[test]
    fn caps_references_at_five() {
        let refs: Vec<_> = (0..10)
            .map(|i| format!(r#"{{"url": "https://example.com/{i}"}}"#))
            .collect();
        let body = format!(
            r#"{{"vulns": [{{"id": "GHSA-1", "references": [{}]}}]}}"#,
            refs.join(",")
        );
        let records = OsvResponseWire::parse(body.as_bytes())
            .expect("valid json")
            .into_vulnerability_records();
        assert_eq!(records[0].references().len(), 5);
    }

    #[test]
    fn falls_back_to_truncated_details_when_summary_empty() {
        let long_details = "x".repeat(300);
        let body = format!(r#"{{"vulns": [{{"id": "GHSA-1", "details": "{long_details}"}}]}}"#);
        let records = OsvResponseWire::parse(body.as_bytes())
            .expect("valid json")
            .into_vulnerability_records();
        assert_eq!(records[0].summary().len(), 200);
    }

    #[test]
    fn picks_highest_severity_when_multiple_scores_present() {
        let body = br#"{
            "vulns": [{
                "id": "GHSA-1",
                "severity": [{"score": "3.1"}, {"score": "9.8/10"}]
            }]
        }"#;
        let records = OsvResponseWire::parse(body)
            .expect("valid json")
            .into_vulnerability_records();
        assert_eq!(records[0].severity(), VulnerabilitySeverity::Critical);
    }
}
