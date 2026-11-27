use serde::{Deserialize, Serialize};

/// What a matching [`RequestRule`](crate::RequestRule) does with the request.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestRuleAction {
    Allow,
    Deny,
}
