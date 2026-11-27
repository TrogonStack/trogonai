/// A command argument does not satisfy the rule [`TerminalArgument`](crate::TerminalArgument) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum TerminalArgumentError {
    #[error("command arguments must not contain NUL")]
    ContainsNul,
}
