use serde::{Deserialize, Serialize};

use crate::{CredentialError, CredentialIssueResponse};

/// The credential broker's reply to a [`CredentialIssueRequest`](crate::CredentialIssueRequest), tagged by
/// `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CredentialIssueOutcome {
    Ok(CredentialIssueResponse),
    Err(CredentialError),
}
