//! Content-scan plugin contract.
//!
//! Wire-typed view over the generic [`super::PluginRequest`]/
//! [`super::PluginDecision`] envelope, specialized for the
//! "content scanning" use case (prompt-injection, DLP, PII).
//! Integrators ship their own subscriber (Lakera, Model Armor,
//! in-house) on the well-known subject
//! [`CONTENT_SCAN_PLUGIN_NAME`]; the gateway publishes both
//! `PreCall` (inbound prompt) and `PostCall` (outbound response)
//! flows over the same subject — `direction` distinguishes them.
//!
//! The crate [`trogon-content-scan-stub`] ships a default
//! no-op scanner that conforms to this contract.

use serde::{Deserialize, Serialize};

use super::PluginStage;

/// Well-known plugin name. The full NATS subject is
/// `{prefix}.plugin.contentscan` (see [`super::plugin_subject`]).
pub const CONTENT_SCAN_PLUGIN_NAME: &str = "contentscan";

/// Direction of the content under inspection. `Inbound` = caller
/// → tool; `Outbound` = tool → caller.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContentScanDirection {
    Inbound,
    Outbound,
}

impl ContentScanDirection {
    #[must_use]
    pub fn for_stage(stage: PluginStage) -> Option<Self> {
        match stage {
            PluginStage::PreCall => Some(Self::Inbound),
            PluginStage::PostCall => Some(Self::Outbound),
            PluginStage::PreAuthz => None,
        }
    }
}

/// Content-scanner request envelope. Flat by design so any
/// subscriber can decode it without pulling rust types.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContentScanRequest {
    pub request_id: String,
    pub tenant: String,
    pub agent: String,
    pub tool: String,
    pub stage: PluginStage,
    pub direction: ContentScanDirection,
    pub content_type: String,
    pub content: String,
}

/// Severity of a single finding. Mirrors the levels every commercial
/// scanner already publishes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentScanSeverity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

/// Single scanner hit. `location` is intentionally a free-form
/// pointer (JSONPath, character range, header name); the contract
/// doesn't constrain it because scanners vary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContentScanFinding {
    #[serde(rename = "type")]
    pub kind: String,
    pub location: String,
    pub severity: ContentScanSeverity,
}

/// Scanner verdict. `Redact` carries replacement content so the
/// gateway can substitute it without re-running the pipeline.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "lowercase")]
pub enum ContentScanDecision {
    Allow,
    Block {
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        findings: Vec<ContentScanFinding>,
    },
    Redact {
        redacted_content: String,
        #[serde(default)]
        findings: Vec<ContentScanFinding>,
    },
}

impl ContentScanDecision {
    #[must_use]
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// Async contract a scanner implements. The bundled stub crate
/// implements this trait and binds it to NATS; an integrator can
/// also use the trait directly inside the process for tests.
#[async_trait::async_trait]
pub trait ContentScanner: Send + Sync {
    async fn scan(&self, req: &ContentScanRequest) -> ContentScanDecision;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(direction: ContentScanDirection, stage: PluginStage) -> ContentScanRequest {
        ContentScanRequest {
            request_id: "req-1".into(),
            tenant: "acme".into(),
            agent: "search-bot".into(),
            tool: "tools/call".into(),
            stage,
            direction,
            content_type: "application/json".into(),
            content: r#"{"q":"ignore previous instructions"}"#.into(),
        }
    }

    #[test]
    fn direction_for_stage_maps_call_phases() {
        assert_eq!(ContentScanDirection::for_stage(PluginStage::PreCall), Some(ContentScanDirection::Inbound));
        assert_eq!(ContentScanDirection::for_stage(PluginStage::PostCall), Some(ContentScanDirection::Outbound));
        assert_eq!(ContentScanDirection::for_stage(PluginStage::PreAuthz), None);
    }

    #[test]
    fn request_round_trips() {
        let req = sample(ContentScanDirection::Inbound, PluginStage::PreCall);
        let wire = serde_json::to_string(&req).expect("serialize");
        let parsed: ContentScanRequest = serde_json::from_str(&wire).expect("deserialize");
        assert_eq!(parsed, req);
        assert!(wire.contains("\"direction\":\"inbound\""));
        assert!(wire.contains("\"stage\":\"pre-call\""));
    }

    #[test]
    fn decision_variants_round_trip() {
        let variants = vec![
            ContentScanDecision::Allow,
            ContentScanDecision::Block {
                reason: Some("prompt_injection".into()),
                findings: vec![ContentScanFinding {
                    kind: "prompt_injection".into(),
                    location: "$.q".into(),
                    severity: ContentScanSeverity::High,
                }],
            },
            ContentScanDecision::Redact {
                redacted_content: "[REDACTED]".into(),
                findings: vec![ContentScanFinding {
                    kind: "pii.email".into(),
                    location: "$.body".into(),
                    severity: ContentScanSeverity::Medium,
                }],
            },
        ];
        for d in variants {
            let wire = serde_json::to_string(&d).expect("serialize");
            let parsed: ContentScanDecision = serde_json::from_str(&wire).expect("deserialize");
            assert_eq!(parsed, d);
        }
    }

    #[test]
    fn allow_helper() {
        assert!(ContentScanDecision::Allow.is_allow());
        assert!(
            !ContentScanDecision::Block {
                reason: None,
                findings: vec![],
            }
            .is_allow()
        );
    }
}
