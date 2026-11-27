use serde::Serialize;

/// E2B's `network` object: what is allowed out, and what is denied out.
#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct NetworkRules {
    #[serde(rename = "allowOut")]
    pub allow_out: Vec<String>,
    #[serde(rename = "denyOut")]
    pub deny_out: Vec<String>,
}
