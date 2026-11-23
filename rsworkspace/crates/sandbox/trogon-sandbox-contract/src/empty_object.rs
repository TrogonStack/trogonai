use serde::{Deserialize, Serialize};

/// Payload for an externally tagged enum variant with no fields.
///
/// It serializes as `{}` so the wire stays object-shaped for JSON Schema `oneOf`, not a bare string,
/// and rejects any field on deserialization.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyObject {}
