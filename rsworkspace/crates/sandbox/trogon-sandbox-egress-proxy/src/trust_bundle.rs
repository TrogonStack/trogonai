//! The CA bundle a workload's TLS clients are pointed at.
//!
//! Clients are told to trust a file in place of their own store, so the file
//! has to carry everything they would have trusted: the CA that signs the
//! intercepted connections, and the roots that verify the hosts the proxy
//! only tunnels.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use tracing::warn;
use trogon_std::env::{ReadEnv, SystemEnv};

mod constants;
use constants::{NODE_EXTRA_CA_CERTS, PEM_LINE_WIDTH};

/// The sandbox CA first, then the system roots, then the roots the parent
/// process trusts through `NODE_EXTRA_CA_CERTS`, then `extra_ca_cert_paths`.
/// Only certificates are copied out of the extra files, so a key kept in the
/// same file never reaches the bundle. A file that is missing or holds no
/// certificate is skipped with a warning.
pub fn workload_trust_bundle(sandbox_ca_pem: &str, extra_ca_cert_paths: &[PathBuf]) -> String {
    let system = rustls_native_certs::load_native_certs();
    for err in &system.errors {
        warn!(error = %err, "failed to load a native root certificate");
    }
    let host_extra = SystemEnv.var_os(NODE_EXTRA_CA_CERTS).map(PathBuf::from);
    assemble(
        sandbox_ca_pem,
        &system.certs,
        host_extra.iter().chain(extra_ca_cert_paths),
    )
}

fn assemble<'a>(
    sandbox_ca_pem: &str,
    system_roots: &[CertificateDer<'static>],
    extra_files: impl IntoIterator<Item = &'a PathBuf>,
) -> String {
    let mut bundle = String::new();
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    push_block(&mut bundle, sandbox_ca_pem.trim());
    for cert in system_roots {
        push_cert(&mut bundle, &mut seen, cert);
    }
    for path in extra_files {
        match certificates_in(path) {
            Ok(certs) if certs.is_empty() => warn!(
                path = %path.display(),
                "the extra CA file has no PEM certificate; skipping it"
            ),
            Ok(certs) => {
                for cert in &certs {
                    push_cert(&mut bundle, &mut seen, cert);
                }
            }
            Err(err) => warn!(
                path = %path.display(),
                error = %err,
                "cannot read the extra CA file; skipping it"
            ),
        }
    }
    bundle
}

fn certificates_in(path: &Path) -> std::io::Result<Vec<CertificateDer<'static>>> {
    let bytes = std::fs::read(path)?;
    Ok(CertificateDer::pem_slice_iter(&bytes).filter_map(Result::ok).collect())
}

fn push_cert(bundle: &mut String, seen: &mut HashSet<Vec<u8>>, cert: &CertificateDer<'_>) {
    if seen.insert(cert.as_ref().to_vec()) {
        push_block(bundle, &pem_encode(cert.as_ref()));
    }
}

fn push_block(bundle: &mut String, block: &str) {
    bundle.push_str(block);
    bundle.push('\n');
}

fn pem_encode(der: &[u8]) -> String {
    let encoded = STANDARD.encode(der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in encoded.as_bytes().chunks(PEM_LINE_WIDTH) {
        pem.push_str(&String::from_utf8_lossy(line));
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----");
    pem
}

#[cfg(test)]
mod tests;
