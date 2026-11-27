use serde::{Deserialize, Serialize};

use crate::range::Range;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}
