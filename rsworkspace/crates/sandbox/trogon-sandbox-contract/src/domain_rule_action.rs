use serde::{Deserialize, Serialize};

use crate::{CredentialTransform, RequestForward};

/// What happens to a matched request for a domain rule.
///
/// It is a single enum rather than two `Option` fields, so that "both transform and forward set" is
/// unrepresentable: a rule carries a `transform` or a `forwardUrl`, never both.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DomainRuleAction {
    #[default]
    PassThrough,
    Transform(CredentialTransform),
    Forward(RequestForward),
}
