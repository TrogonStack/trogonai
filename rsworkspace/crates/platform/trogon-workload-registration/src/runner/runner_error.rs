use std::{env, io};

use trogon_workload_registration::RegistrationError;

#[derive(thiserror::Error)]
pub enum RunnerError {
    #[error("administrative arguments are invalid")]
    Arguments(#[source] clap::Error),
    #[error("approval public key encoding is invalid")]
    PublicKeyEncoding(#[source] hex::FromHexError),
    #[error("approval public key is invalid")]
    PublicKey(#[source] ed25519_dalek::SignatureError),
    #[error("administrative configuration is invalid")]
    Configuration(#[source] Box<RegistrationError>),
    #[error("administrative environment is invalid")]
    Environment(#[source] env::VarError),
    #[error("an explicit NATS URL is required")]
    ExplicitNats,
    #[error("verified TLS is required")]
    Transport,
    #[error("authenticated NATS is required")]
    Authentication,
    #[error("controller input is not a supported pipe or complete frame")]
    Input(#[source] io::Error),
    #[error("controller frame exceeds the supported bounds")]
    FrameBounds,
    #[error("controller frame deadline elapsed")]
    FrameTimeout,
    #[error("controller approval is invalid")]
    Approval(#[source] Box<RegistrationError>),
    #[error("emergency withdrawal requires a complete denial")]
    ExpectedDenial,
    #[error("protected publication failed")]
    Producer(#[source] Box<RegistrationError>),
    #[error("protected publication deadline elapsed")]
    OperationTimeout,
    #[error("approved authority expired")]
    Expired,
    #[error("administrative execution was cancelled")]
    Cancelled,
    #[error("signal handling could not be installed")]
    Signal(#[source] io::Error),
    #[error("static help could not be written")]
    Output(#[source] io::Error),
}

impl RunnerError {
    pub fn configuration(error: RegistrationError) -> Self {
        Self::Configuration(Box::new(error))
    }

    pub fn producer(error: RegistrationError) -> Self {
        Self::Producer(Box::new(error))
    }

    pub fn approval(error: RegistrationError) -> Self {
        Self::Approval(Box::new(error))
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Arguments(_)
            | Self::PublicKeyEncoding(_)
            | Self::PublicKey(_)
            | Self::Configuration(_)
            | Self::Environment(_)
            | Self::ExplicitNats
            | Self::Transport
            | Self::Authentication
            | Self::Signal(_)
            | Self::Output(_) => 2,
            Self::Input(_) | Self::FrameBounds | Self::FrameTimeout => 3,
            Self::Approval(_) | Self::ExpectedDenial => 4,
            Self::Producer(_) | Self::OperationTimeout => 5,
            Self::Expired | Self::Cancelled => 6,
        }
    }
}

impl std::fmt::Debug for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerError")
            .field("exit_code", &self.exit_code())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
