//! Reads the caller out of a verified client certificate (ADR#0085 Decision 3).
//!
//! The caller is the certificate's single URI subject alternative name and
//! nothing else: not the subject DN, not a DNS SAN, not anything the client
//! sends after the handshake. Zero or several URI SANs name no caller.

use x509_parser::certificate::X509Certificate;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::FromDer;

use crate::agent_id::AgentId;
use crate::agent_uri::{AgentUriError, agent_id_from_uri};
use crate::trust_domain::TrustDomain;

pub fn agent_id_from_certificate(der: &[u8], trust_domain: &TrustDomain) -> Result<AgentId, PeerIdentityError> {
    let (rest, certificate) = X509Certificate::from_der(der).map_err(|_| PeerIdentityError::Unparsable)?;
    if !rest.is_empty() {
        return Err(PeerIdentityError::Unparsable);
    }
    let san = certificate
        .subject_alternative_name()
        .map_err(|_| PeerIdentityError::Unparsable)?
        .ok_or(PeerIdentityError::NoUriSan)?;
    let mut uris = san.value.general_names.iter().filter_map(|name| match name {
        GeneralName::URI(uri) => Some(*uri),
        _ => None,
    });
    let uri = uris.next().ok_or(PeerIdentityError::NoUriSan)?;
    if uris.next().is_some() {
        return Err(PeerIdentityError::MultipleUriSans);
    }
    agent_id_from_uri(uri, trust_domain).map_err(PeerIdentityError::Uri)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerIdentityError {
    #[error("client certificate could not be parsed")]
    Unparsable,
    #[error("client certificate has no URI subject alternative name")]
    NoUriSan,
    #[error("client certificate has more than one URI subject alternative name")]
    MultipleUriSans,
    #[error("client certificate URI does not name an agent")]
    Uri(#[source] AgentUriError),
}

#[cfg(test)]
mod tests;
