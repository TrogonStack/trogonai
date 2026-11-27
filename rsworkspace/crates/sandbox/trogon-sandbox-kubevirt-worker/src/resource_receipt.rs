use serde::{Deserialize, Serialize};

/// The exact identity of one Kubernetes object a lease created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceReceipt {
    pub namespace: String,
    pub name: String,
    pub uid: String,
}
