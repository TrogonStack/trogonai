//! Mission wire types per draft section "Mission": creation, approval, log,
//! completion, management, status errors, and the `AAuth-Mission` request header.

use serde::{Deserialize, Serialize};

/// A tool the agent wants to use, or that the PS has pre-approved, per
/// "Mission Creation" and "Mission Approval".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Mission creation proposal sent by the agent to the PS's `mission_endpoint`,
/// per "Mission Creation".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionProposal {
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<MissionTool>>,
}

/// The mission blob returned by the PS on approval, per "Mission Approval". This is
/// the exact JSON whose bytes are hashed to produce `s256` everywhere it appears;
/// callers MUST preserve the exact response bytes rather than re-serializing this
/// struct for hashing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionBlob {
    pub approver: String,
    pub agent: String,
    pub approved_at: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_tools: Option<Vec<MissionTool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
}

/// Mission log entry kinds accumulated by the PS during a mission, per
/// "Mission Log". The draft describes the log's contents in prose without a
/// concrete wire example, so this enum is modeled conservatively from the
/// enumerated interaction types (token requests, permission requests/responses,
/// audit records, interaction requests, and clarification chats).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MissionLogEntry {
    TokenRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        justification: Option<String>,
    },
    PermissionRequest {
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    PermissionResponse {
        permission: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    AuditRecord {
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    InteractionRequest {
        #[serde(rename = "type")]
        type_: String,
    },
    ClarificationChat {
        clarification: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clarification_response: Option<String>,
    },
}

/// Mission completion summary sent via the interaction endpoint (`type: completion`),
/// per "Mission Completion".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionCompletion {
    pub summary: String,
    pub mission: super::MissionRef,
}

/// Mission lifecycle state per "Mission Management".
pub use super::error::MissionStatus;
/// Mission status error body per "Mission Status Errors".
pub use super::error::MissionStatusError;

/// Parsed `AAuth-Mission` request header per "AAuth-Mission Request Header".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionHeader {
    pub approver: String,
    pub s256: String,
}

impl MissionHeader {
    /// Render the canonical wire form for the `AAuth-Mission` header value.
    #[must_use]
    pub fn to_header_value(&self) -> String {
        format!("approver=\"{}\"; s256=\"{}\"", self.approver, self.s256)
    }

    /// Parse the value of an `AAuth-Mission` header into `{approver, s256}`.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let mut approver: Option<String> = None;
        let mut s256: Option<String> = None;
        for seg in raw.split(';') {
            let seg = seg.trim();
            if seg.is_empty() {
                continue;
            }
            let (k, v) = seg.split_once('=')?;
            let key = k.trim();
            let val = strip_quotes(v.trim());
            match key {
                "approver" => approver = Some(val),
                "s256" => s256 = Some(val),
                _ => {}
            }
        }
        Some(MissionHeader {
            approver: approver?,
            s256: s256?,
        })
    }
}

fn strip_quotes(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes.first() == Some(&b'"') && bytes.last() == Some(&b'"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

impl From<MissionHeader> for super::MissionRef {
    fn from(header: MissionHeader) -> Self {
        super::MissionRef {
            approver: header.approver,
            s256: header.s256,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mission_proposal_serde_matches_draft_example() {
        let proposal = MissionProposal {
            description: "# Plan Japan Vacation".into(),
            tools: Some(vec![
                MissionTool {
                    name: "WebSearch".into(),
                    description: Some("Search the web".into()),
                },
                MissionTool {
                    name: "BookFlight".into(),
                    description: Some("Book flights".into()),
                },
            ]),
        };
        let json = serde_json::to_value(&proposal).unwrap();
        assert_eq!(json["tools"][0]["name"], "WebSearch");
        let back: MissionProposal = serde_json::from_value(json).unwrap();
        assert_eq!(back, proposal);
    }

    #[test]
    fn mission_blob_serde_matches_draft_example() {
        let raw = serde_json::json!({
            "approver": "https://ps.example",
            "agent": "aauth:assistant@agent.example",
            "approved_at": "2026-04-07T14:30:00Z",
            "description": "# Plan Japan Vacation",
            "approved_tools": [
                {"name": "WebSearch", "description": "Search the web"},
                {"name": "Read", "description": "Read files and web pages"}
            ],
            "capabilities": ["interaction", "payment"]
        });
        let blob: MissionBlob = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(blob.approver, "https://ps.example");
        assert_eq!(
            blob.capabilities.as_ref().unwrap(),
            &vec!["interaction".to_string(), "payment".to_string()]
        );
        let round = serde_json::to_value(&blob).unwrap();
        assert_eq!(round, raw);
    }

    #[test]
    fn mission_blob_optional_fields_omitted_when_absent() {
        let blob = MissionBlob {
            approver: "https://ps.example".into(),
            agent: "aauth:assistant@agent.example".into(),
            approved_at: "2026-04-07T14:30:00Z".into(),
            description: "desc".into(),
            approved_tools: None,
            capabilities: None,
        };
        let json = serde_json::to_value(&blob).unwrap();
        assert!(json.get("approved_tools").is_none());
        assert!(json.get("capabilities").is_none());
    }

    #[test]
    fn mission_completion_serde_matches_draft_example() {
        let completion = MissionCompletion {
            summary: "# Japan Trip Booked".into(),
            mission: super::super::MissionRef {
                approver: "https://ps.example".into(),
                s256: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into(),
            },
        };
        let json = serde_json::to_value(&completion).unwrap();
        assert_eq!(json["mission"]["approver"], "https://ps.example");
        let back: MissionCompletion = serde_json::from_value(json).unwrap();
        assert_eq!(back, completion);
    }

    #[test]
    fn mission_header_parse_and_render_round_trip() {
        let raw = "approver=\"https://ps.example\"; s256=\"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk\"";
        let header = MissionHeader::parse(raw).unwrap();
        assert_eq!(header.approver, "https://ps.example");
        assert_eq!(header.s256, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(header.to_header_value(), raw);
    }

    #[test]
    fn mission_header_parse_skips_empty_segments_from_stray_semicolons() {
        let raw = "approver=\"https://ps.example\";; s256=\"hash\";";
        let header = MissionHeader::parse(raw).unwrap();
        assert_eq!(header.approver, "https://ps.example");
        assert_eq!(header.s256, "hash");
    }

    #[test]
    fn mission_header_parse_ignores_unknown_parameters() {
        let raw = "approver=\"https://ps.example\"; s256=\"hash\"; future-param=\"x\"";
        let header = MissionHeader::parse(raw).unwrap();
        assert_eq!(header.approver, "https://ps.example");
        assert_eq!(header.s256, "hash");
    }

    #[test]
    fn mission_header_parse_accepts_unquoted_values() {
        let raw = "approver=https://ps.example; s256=hash";
        let header = MissionHeader::parse(raw).unwrap();
        assert_eq!(header.approver, "https://ps.example");
        assert_eq!(header.s256, "hash");
    }

    #[test]
    fn mission_header_converts_into_mission_ref() {
        let header = MissionHeader {
            approver: "https://ps.example".into(),
            s256: "hash".into(),
        };
        let mission_ref: super::super::MissionRef = header.into();
        assert_eq!(mission_ref.approver, "https://ps.example");
        assert_eq!(mission_ref.s256, "hash");
    }

    #[test]
    fn mission_log_entry_variants_serde_round_trip() {
        let entries = vec![
            MissionLogEntry::TokenRequest {
                justification: Some("Find available meeting times".into()),
            },
            MissionLogEntry::PermissionRequest {
                action: "SendEmail".into(),
                description: None,
            },
            MissionLogEntry::PermissionResponse {
                permission: "granted".into(),
                reason: None,
            },
            MissionLogEntry::AuditRecord {
                action: "WebSearch".into(),
                description: Some("Searched for flights".into()),
            },
            MissionLogEntry::InteractionRequest {
                type_: "completion".into(),
            },
            MissionLogEntry::ClarificationChat {
                clarification: "Why do you need write access?".into(),
                clarification_response: Some("To create invites".into()),
            },
        ];
        for entry in entries {
            let json = serde_json::to_string(&entry).unwrap();
            let back: MissionLogEntry = serde_json::from_str(&json).unwrap();
            assert_eq!(back, entry);
        }
    }
}
