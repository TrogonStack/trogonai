use super::v8_string_length;
use crate::{constants::V8_MAX_STRING_LENGTH, settings_resolve_error::ResolveSettingsError};

#[test]
fn printing_the_resolved_object_fails_past_the_longest_string_v8_holds() {
    assert!(v8_string_length(V8_MAX_STRING_LENGTH).is_ok());
    assert!(matches!(
        v8_string_length(V8_MAX_STRING_LENGTH + 1),
        Err(ResolveSettingsError::InvalidStringLength)
    ));
}
