use crate::ulid::Ulid;

/// Whether `value` is exactly the canonical text of a ULID.
pub(crate) fn is_opaque_id(value: &str) -> bool {
    Ulid::parse(value).is_ok_and(|parsed| parsed.to_string() == value)
}
