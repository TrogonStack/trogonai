/// What a request does when its spelled target and its normalized target differ.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash)]
pub enum AmbiguousTargets {
    /// Refuse the request.
    #[default]
    Refuse,
    /// Use the target as spelled.
    AsSpelled,
    /// Use the normalized target.
    Normalize,
}
