use crate::constants::{ENV_NATS_CERT, ENV_NATS_KEY};
use crate::credential_value::Location;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCertificate {
    pub cert: Location,
    pub key: Location,
}

/// A client certificate configured without its counterpart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncompleteClientCertificate {
    MissingCert,
    MissingKey,
}

impl IncompleteClientCertificate {
    pub fn missing_setting(self) -> &'static str {
        match self {
            Self::MissingCert => ENV_NATS_CERT,
            Self::MissingKey => ENV_NATS_KEY,
        }
    }

    pub fn present_setting(self) -> &'static str {
        match self {
            Self::MissingCert => ENV_NATS_KEY,
            Self::MissingKey => ENV_NATS_CERT,
        }
    }
}

/// Transport security for a connection. TLS is the default and verifies the
/// server against the system roots, or against `ca` in their place, and a
/// client certificate adds mutual TLS. `Disabled` is the only way to connect
/// without TLS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NatsTls {
    Disabled,
    Enabled {
        ca: Option<Location>,
        client_cert: Option<ClientCertificate>,
    },
}

impl Default for NatsTls {
    fn default() -> Self {
        Self::Enabled {
            ca: None,
            client_cert: None,
        }
    }
}

impl NatsTls {
    pub fn description(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Enabled { client_cert: None, .. } => "TLS",
            Self::Enabled {
                client_cert: Some(_), ..
            } => "mutual TLS",
        }
    }
}

/// TLS settings that cannot be honored as written. Connecting reports them
/// instead of choosing a transport on the caller's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsMisconfiguration {
    IncompleteClientCertificate(IncompleteClientCertificate),
    UnrecognizedDisabledValue(String),
    DisabledAlongside(&'static str),
}

#[cfg(test)]
mod tests;
