/// What became of a release offered to the cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReleaseHold {
    /// It is held.
    Held,
    /// It is not held: nothing is held right now, it was overtaken on its
    /// way, or its hold is already over.
    Declined,
    /// It is not held because every slot holds a copy that still serves.
    Full,
}
