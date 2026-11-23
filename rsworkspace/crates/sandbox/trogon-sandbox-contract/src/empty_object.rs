use std::collections::BTreeMap;

use serde::de::{self, IgnoredAny};
use serde::{Deserialize, Deserializer, Serialize};

/// Payload for an externally tagged enum variant with no fields.
///
/// It serializes as `{}` so the wire stays object-shaped for JSON Schema `oneOf`, not a bare string.
/// Deserialization accepts only an object and rejects any field.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize)]
pub struct EmptyObject {}

impl<'de> Deserialize<'de> for EmptyObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = BTreeMap::<String, IgnoredAny>::deserialize(deserializer)?;
        match fields.keys().next() {
            None => Ok(Self {}),
            Some(field) => Err(de::Error::unknown_field(field, &[])),
        }
    }
}
