use serde::{Deserialize, Serialize};

use crate::BodyTruncation;

/// The part of a request body shown to a request decider.
///
/// `data` is base64. `cut` is `Truncated`, a JSON `true`, when the body went past the cap and was not shown whole,
/// and the field is omitted when the body is complete.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestDecideBody {
    pub data: String,
    #[serde(default, skip_serializing_if = "BodyTruncation::is_complete")]
    pub cut: BodyTruncation,
}
