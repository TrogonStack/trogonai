use crate::{ClaudeAgentOptions, Error};

#[derive(Debug)]
pub struct ClaudeSdkClient {
    _private: (),
}

impl ClaudeSdkClient {
    /// Attempts to create a connected client using the supplied options.
    ///
    /// # Errors
    /// This API-only scaffold always returns [`Error::NotImplemented`] without
    /// starting a process or exchanging protocol messages.
    pub async fn connect(_options: ClaudeAgentOptions) -> Result<Self, Error> {
        Err(Error::NotImplemented)
    }

    /// Consumes the client through the asynchronous disconnect entry point.
    ///
    /// # Errors
    /// This API-only scaffold always returns [`Error::NotImplemented`]; runtime
    /// cleanup has not been implemented.
    pub async fn disconnect(self) -> Result<(), Error> {
        Err(Error::NotImplemented)
    }
}
