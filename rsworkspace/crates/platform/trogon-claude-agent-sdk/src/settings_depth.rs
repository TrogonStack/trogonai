//! How deeply nested a settings document the TypeScript dialect reads may be.
//!
//! Node reads and merges a settings document recursively, and fails with `RangeError: Maximum call stack size
//! exceeded` where V8 runs out of stack. Where that happens depends on the tier V8 runs each function in, which
//! functions it has compiled, the caller's own stack and everything the process ran before, so the same document
//! fails at different depths in different runs. The SDK does not model it. It picks one bound instead, and every
//! document of the TypeScript dialect at that depth or deeper fails the resolve the same way on every host, in every
//! caller context and after any history of earlier resolves.

use serde_json::{Map, Value};

use crate::{
    constants::SETTINGS_DEPTH_BOUND,
    settings_parse_deep::{nests_past, parses_as_object},
    settings_resolve_error::ResolveSettingsError,
};

/// The depth of the object holding `entries`, which stops counting at [`SETTINGS_DEPTH_BOUND`] and returns it.
pub(crate) fn object_depth(entries: &Map<String, Value>) -> usize {
    deepest(entries.values(), 1)
}

/// The depth of the deepest of `roots`, each of them `above` levels down, without recursion.
fn deepest<'a>(roots: impl IntoIterator<Item = &'a Value>, above: usize) -> usize {
    let mut deepest = above;
    let mut pending: Vec<(&Value, usize)> = roots.into_iter().map(|root| (root, above)).collect();
    while let Some((value, outside)) = pending.pop() {
        let level = outside.saturating_add(1);
        match value {
            Value::Array(items) => pending.extend(items.iter().map(|item| (item, level))),
            Value::Object(entries) => pending.extend(entries.values().map(|item| (item, level))),
            _ => continue,
        }
        deepest = deepest.max(level);
        if deepest >= SETTINGS_DEPTH_BOUND {
            return SETTINGS_DEPTH_BOUND;
        }
    }
    deepest
}

/// Fails with [`ResolveSettingsError::MaximumCallStack`] for a settings document handed over as an object that is
/// nested [`SETTINGS_DEPTH_BOUND`] levels or deeper.
pub(crate) fn check_object(entries: &Map<String, Value>) -> Result<(), ResolveSettingsError> {
    if object_depth(entries) >= SETTINGS_DEPTH_BOUND {
        return Err(ResolveSettingsError::MaximumCallStack);
    }
    Ok(())
}

/// Fails with [`ResolveSettingsError::MaximumCallStack`] for settings text that `JSON.parse` reads as an object nested
/// [`SETTINGS_DEPTH_BOUND`] levels or deeper, before anything builds the value. Text `JSON.parse` rejects, or reads as
/// anything but an object, is no document and passes.
pub(crate) fn check_text(text: &str) -> Result<(), ResolveSettingsError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if nests_past(text, SETTINGS_DEPTH_BOUND - 1) && parses_as_object(text) == Some(true) {
        return Err(ResolveSettingsError::MaximumCallStack);
    }
    Ok(())
}

#[cfg(test)]
#[path = "settings_depth_tests.rs"]
mod tests;
