use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{CustodyToken, WarmCandidate};

/// A claim on a warm VM that was written down before the cluster was asked to honor it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WarmClaimIntent {
    pub candidate: WarmCandidate,
    pub token: CustodyToken,
    pub labels: BTreeMap<String, String>,
}
