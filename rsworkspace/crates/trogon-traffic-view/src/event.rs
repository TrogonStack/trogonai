use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActChainHop {
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub wkl: String,
    pub iat: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficSource {
    Gateway,
    Sts,
    Registry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficDecision {
    Allow,
    Deny,
    Rewrite,
    RateLimited,
    Unknown,
}

/// Normalized row written to `agent_traffic_events` and exported to SIEM.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrafficEvent {
    pub event_id: String,
    pub ts: DateTime<Utc>,
    pub tenant: String,
    pub caller_sub: Option<String>,
    pub caller_wkl: Option<String>,
    pub target_aud: Option<String>,
    pub purpose: Option<String>,
    pub scope: Option<String>,
    pub outcome: String,
    pub reason: Option<String>,
    pub act_chain: Option<Vec<ActChainHop>>,
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    pub source: TrafficSource,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TrafficQueryFilter {
    pub tenant: Option<String>,
    pub agent_id: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub limit: Option<u32>,
}

impl fmt::Display for TrafficSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gateway => f.write_str("gateway"),
            Self::Sts => f.write_str("sts"),
            Self::Registry => f.write_str("registry"),
        }
    }
}

impl TrafficDecision {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Rewrite => "rewrite",
            Self::RateLimited => "rate_limited",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "allow" | "success" | "ok" => Self::Allow,
            "deny" | "err" | "error" => Self::Deny,
            "rewrite" | "redact" => Self::Rewrite,
            "rate_limited" => Self::RateLimited,
            _ => Self::Unknown,
        }
    }
}
