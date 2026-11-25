use serde::{Deserialize, Serialize};

use crate::{ClaimName, ExtractPattern};

/// How a JSON web token credential is masked.
///
/// With named claims, each one that is a string in the token gets a stand-in of its own, and at least one must. An
/// optional pattern narrows where in a file the tokens are looked for. Both fields are omitted from the wire when
/// empty.
#[derive(Debug, Clone, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct JwtMasking {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mask_claims: Vec<ClaimName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<ExtractPattern>,
}
