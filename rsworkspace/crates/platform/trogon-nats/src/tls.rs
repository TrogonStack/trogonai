use crate::credential_value::Location;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCertificate {
    pub cert: Location,
    pub key: Location,
}

/// TLS material; a client certificate only applies when both its certificate
/// and key are present.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NatsTls {
    client_certificate: Option<ClientCertificate>,
    root_ca: Option<Location>,
}

impl NatsTls {
    pub fn new(cert: Option<Location>, key: Option<Location>, root_ca: Option<Location>) -> Self {
        Self {
            client_certificate: cert.zip(key).map(|(cert, key)| ClientCertificate { cert, key }),
            root_ca,
        }
    }

    pub fn client_certificate(&self) -> Option<&ClientCertificate> {
        self.client_certificate.as_ref()
    }

    pub fn root_ca(&self) -> Option<&Location> {
        self.root_ca.as_ref()
    }

    pub fn is_configured(&self) -> bool {
        self.client_certificate.is_some() || self.root_ca.is_some()
    }
}

#[cfg(test)]
mod tests;
