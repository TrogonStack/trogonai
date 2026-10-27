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

/// TLS material; a client certificate needs both its certificate and key, and
/// setting either one alone is reported instead of ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NatsTls {
    cert: Option<Location>,
    key: Option<Location>,
    root_ca: Option<Location>,
}

impl NatsTls {
    pub fn new(cert: Option<Location>, key: Option<Location>, root_ca: Option<Location>) -> Self {
        Self { cert, key, root_ca }
    }

    pub fn client_certificate(&self) -> Result<Option<ClientCertificate>, IncompleteClientCertificate> {
        match (&self.cert, &self.key) {
            (Some(cert), Some(key)) => Ok(Some(ClientCertificate {
                cert: cert.clone(),
                key: key.clone(),
            })),
            (Some(_), None) => Err(IncompleteClientCertificate::MissingKey),
            (None, Some(_)) => Err(IncompleteClientCertificate::MissingCert),
            (None, None) => Ok(None),
        }
    }

    pub fn root_ca(&self) -> Option<&Location> {
        self.root_ca.as_ref()
    }

    pub fn is_configured(&self) -> bool {
        self.cert.is_some() || self.key.is_some() || self.root_ca.is_some()
    }
}

#[cfg(test)]
mod tests;
