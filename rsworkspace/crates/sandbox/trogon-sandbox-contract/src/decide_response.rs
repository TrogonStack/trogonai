use serde::{Deserialize, Serialize};

use crate::VerdictScope;

/// Reply to a [`DecideRequest`](crate::DecideRequest), tagged by `verdict`.
///
/// Anything else, including an unknown verdict or an extra field, is treated by the proxy as a deny.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verdict", deny_unknown_fields)]
pub enum DecideResponse {
    Allow { scope: VerdictScope },
    Deny { scope: VerdictScope },
}
