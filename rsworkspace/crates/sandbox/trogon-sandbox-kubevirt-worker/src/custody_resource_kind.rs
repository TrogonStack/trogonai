use serde::{Deserialize, Serialize};

/// The kind of object a create intent was written for.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CustodyResourceKind {
    VirtualMachine,
    DataVolume,
}
