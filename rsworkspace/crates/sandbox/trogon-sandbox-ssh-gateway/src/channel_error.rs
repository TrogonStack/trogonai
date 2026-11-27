use std::num::NonZeroUsize;

use async_nats::SubscribeError;
use trogon_sandbox_contract::{TerminalArgumentError, TerminalIdError, TerminalInputError, TerminalResponse};

/// Why the bridge could not serve one SSH channel.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ChannelError {
    #[error("session is at its limit of {max} channels")]
    AtLimit { max: NonZeroUsize },
    #[error("invalid command: {0}")]
    Command(#[from] TerminalArgumentError),
    #[error("invalid terminal id: {0}")]
    TerminalId(#[from] TerminalIdError),
    #[error("output subscribe failed: {0}")]
    Subscribe(#[from] SubscribeError),
    #[error("terminal not opened: {0:?}")]
    NotOpened(TerminalResponse),
    #[error("invalid terminal input: {0}")]
    Input(#[from] TerminalInputError),
    #[error("terminal refused the request: {0:?}")]
    Refused(TerminalResponse),
}
