/// An environment does not satisfy the rules [`CommandEnv`](crate::CommandEnv) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum CommandEnvError {
    #[error("environment key {} is reserved", crate::constants::EXECUTION_ID_ENV)]
    ReservedKey,
}
