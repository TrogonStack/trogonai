use crate::{ClaudeAgentOptions, Error};

#[derive(Debug)]
pub struct ClaudeSdkClient {
    _private: (),
}

impl ClaudeSdkClient {
    pub async fn connect(_options: ClaudeAgentOptions) -> Result<Self, Error> {
        Err(Error::NotImplemented)
    }

    pub async fn disconnect(self) -> Result<(), Error> {
        Err(Error::NotImplemented)
    }
}
