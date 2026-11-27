use super::constants::{ANONYMOUS_PREFIX, NAMED_PREFIX};

/// Whether a name on the Substrate side is one of ours.
///
/// The reaper deletes actors this returns `true` for and nothing else, so an operator's
/// hand-made actor in the same atespace survives.
pub fn is_managed(name: &str) -> bool {
    name.starts_with(ANONYMOUS_PREFIX) || name.starts_with(NAMED_PREFIX)
}
