/// A path accepted by the lexical check, with the prefixes the symlink check must inspect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfinedPath {
    /// Absolute, normalized path to send to the sandbox.
    pub path: String,
    /// Every prefix below the matched root, shortest first, ending with `path` itself.
    pub below_root: Vec<String>,
}
