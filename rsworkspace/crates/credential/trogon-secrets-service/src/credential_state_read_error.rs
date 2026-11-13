/// Why the credential's state could not be read.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialStateReadError {
    /// The credential stream could not be reached or read.
    #[error("credential stream is unavailable: {message}")]
    Unavailable { message: String },
    /// The credential stream holds events that do not fold.
    #[error("credential stream does not fold: {message}")]
    Undecodable { message: String },
}

#[cfg(test)]
mod tests;
