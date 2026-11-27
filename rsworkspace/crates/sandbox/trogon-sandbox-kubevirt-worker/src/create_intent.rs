use serde::{Deserialize, Serialize};

use crate::{BoundedName, CustodyResourceKind, CustodyToken};

/// A create that was written down before the cluster was asked to perform it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateIntent {
    pub kind: CustodyResourceKind,
    pub name: BoundedName,
    pub nonce: CustodyToken,
}
