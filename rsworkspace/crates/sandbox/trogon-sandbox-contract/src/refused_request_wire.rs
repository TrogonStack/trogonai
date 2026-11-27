use serde::{Deserialize, Serialize};

/// A refused request as it crosses the wire, before the target's query and fragment are cut.
#[derive(Serialize, Deserialize)]
pub(crate) struct RefusedRequestWire {
    pub(crate) method: String,
    pub(crate) target: String,
    pub(crate) reason: String,
}
