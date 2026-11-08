#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("the Claude Code runtime is not implemented")]
    NotImplemented,
}
