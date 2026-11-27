use std::io::{Cursor, Read};
use std::path::Path;

use tokio_rustls::rustls::pki_types::CertificateDer;

use super::RouterTlsError;
use super::constants::MAX_PEM_BYTES;

pub(super) fn read_pem(path: &Path, description: &str) -> Result<Vec<u8>, RouterTlsError> {
    let file = std::fs::File::open(path).map_err(|source| RouterTlsError::Open {
        description: description.to_string(),
        source,
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_PEM_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| RouterTlsError::Read {
            description: description.to_string(),
            source,
        })?;
    if bytes.len() as u64 > MAX_PEM_BYTES {
        return Err(RouterTlsError::PemTooLarge {
            description: description.to_string(),
        });
    }
    Ok(bytes)
}

pub(super) fn certificates(bytes: &[u8], description: &str) -> Result<Vec<CertificateDer<'static>>, RouterTlsError> {
    let certs = rustls_pemfile::certs(&mut Cursor::new(bytes))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| RouterTlsError::InvalidPem {
            description: description.to_string(),
        })?;
    if certs.is_empty() {
        return Err(RouterTlsError::NoCertificates {
            description: description.to_string(),
        });
    }
    Ok(certs)
}
