/// Why a terminal's command line could not be built.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub(crate) enum CommandLineError {
    #[error("invalid environment variable name {0:?}")]
    InvalidEnvName(String),
}
