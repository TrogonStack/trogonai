//! The TLS side of `http.client.HTTPSConnection` under the context `urllib.request` builds by default,
//! `ssl.create_default_context()` with `http/1.1` as the only ALPN protocol: TLS 1.2 or later, the certificate chain
//! checked against the trusted roots, then the host name or IP address checked against the certificate. `urlopen`
//! builds the context with its opener, so the trusted roots are read on the first retrieval, whatever its scheme.
//!
//! Python reports a rejected certificate with OpenSSL's text. The common failures keep that text: a self-signed
//! certificate, a missing issuer, a name or address mismatch, an expired or not yet valid certificate. Other
//! certificate failures carry the text of `rustls` instead, and the roots come from the operating system store rather
//! than OpenSSL's.
//!
//! A server that breaks the protocol before the handshake is encrypted gets the `ssl.SSLError` text OpenSSL 3.5 gives
//! it: the gate in front of the socket applies OpenSSL's record, alert and server hello rules before `rustls` reads.

use crate::RemoteSchemaError;
use crate::adapters::EnvironmentVariable;
use crate::constants::{EOF, RECORD_LAYER_FAILURE, SSL_LOCATION};
use crate::python_urlopen_text::os_error_text;
use crate::python_urlopen_tls_gate::{Gate, GateFailureError, alert_text};
use crate::remote_schema_error::url_error;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::{Resumption, Tls12Resumption, WebPkiServerVerifier};
use rustls::crypto::aws_lc_rs::kx_group::{SECP256R1, SECP384R1, X25519, X25519MLKEM768};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct, Error, OtherError, RootCertStore,
    SignatureScheme, StreamOwned,
};
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;

/// Why the certificate was rejected, in OpenSSL's words.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct VerifyFailedError(String);

/// A socket the request is written to and the response read from.
pub(crate) trait Duplex: Read + Write + Send {}

impl<T: Read + Write + Send> Duplex for T {}

/// One DER element: its tag, its contents and the bytes after it.
fn read_tlv(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, after) = input.split_first()?;
    let (&first, mut after) = after.split_first()?;
    let length = if first < 0x80 {
        usize::from(first)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 {
            return None;
        }
        let (bytes, tail) = after.split_at_checked(count)?;
        after = tail;
        bytes
            .iter()
            .fold(0usize, |length, &byte| (length << 8) | usize::from(byte))
    };
    let (contents, rest) = after.split_at_checked(length)?;
    Some((tag, contents, rest))
}

/// The issuer and subject names of a certificate.
fn names(der: &[u8]) -> Option<(&[u8], &[u8])> {
    let (_, certificate, _) = read_tlv(der)?;
    let (_, tbs, _) = read_tlv(certificate)?;
    let (tag, _, mut rest) = read_tlv(tbs)?;
    if tag == 0xa0 {
        (_, _, rest) = read_tlv(rest)?;
    }
    let (_, _, rest) = read_tlv(rest)?;
    let (_, issuer, rest) = read_tlv(rest)?;
    let (_, _, rest) = read_tlv(rest)?;
    let (_, subject, _) = read_tlv(rest)?;
    Some((issuer, subject))
}

/// Whether the certificate names itself as its issuer, which OpenSSL reports as self-signed.
fn is_self_issued(der: &[u8]) -> bool {
    names(der).is_some_and(|(issuer, subject)| issuer == subject)
}

/// The OpenSSL reason for a rejected certificate.
fn reason(
    error: &Error,
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
    server_name: &ServerName<'_>,
) -> String {
    match error {
        Error::InvalidCertificate(CertificateError::UnknownIssuer) => {
            if intermediates.is_empty() && is_self_issued(end_entity) {
                "self-signed certificate".to_owned()
            } else if intermediates.iter().any(|certificate| is_self_issued(certificate)) {
                "self-signed certificate in certificate chain".to_owned()
            } else {
                "unable to get local issuer certificate".to_owned()
            }
        }
        Error::InvalidCertificate(
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
        ) => match server_name {
            ServerName::IpAddress(_) => {
                format!(
                    "IP address mismatch, certificate is not valid for '{}'.",
                    server_name.to_str()
                )
            }
            _ => format!(
                "Hostname mismatch, certificate is not valid for '{}'.",
                server_name.to_str()
            ),
        },
        Error::InvalidCertificate(CertificateError::Expired | CertificateError::ExpiredContext { .. }) => {
            "certificate has expired".to_owned()
        }
        Error::InvalidCertificate(CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. }) => {
            "certificate is not yet valid".to_owned()
        }
        other => other.to_string(),
    }
}

fn rejected(text: String) -> Error {
    Error::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(VerifyFailedError(text)))))
}

/// The `webpki` verifier with OpenSSL's reasons. With no trusted roots every chain has an unknown issuer.
#[derive(Debug)]
struct OpenSslReasons {
    inner: Option<Arc<WebPkiServerVerifier>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for OpenSslReasons {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let verified = match &self.inner {
            Some(inner) => inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now),
            None => Err(Error::InvalidCertificate(CertificateError::UnknownIssuer)),
        };
        verified.map_err(|error| rejected(reason(&error, end_entity, intermediates, server_name)))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// The `URLError` a failed handshake raises.
fn handshake_failure(error: &io::Error) -> RemoteSchemaError {
    if let Some(refused) = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<GateFailureError>())
    {
        return url_error(&format!("{} {SSL_LOCATION}", refused.text()));
    }
    match error.get_ref().and_then(|inner| inner.downcast_ref::<Error>()) {
        Some(Error::InvalidCertificate(CertificateError::Other(OtherError(reason)))) => url_error(&format!(
            "[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: {reason} {SSL_LOCATION}"
        )),
        Some(Error::DecryptError) => url_error(&format!("{RECORD_LAYER_FAILURE} {SSL_LOCATION}")),
        Some(Error::AlertReceived(alert)) => url_error(&format!("{} {SSL_LOCATION}", alert_text(u8::from(*alert)))),
        Some(other) => url_error(&format!("[SSL] {other} {SSL_LOCATION}")),
        None if error.kind() == io::ErrorKind::UnexpectedEof => url_error(&format!("{EOF} {SSL_LOCATION}")),
        None => url_error(&os_error_text(error, None)),
    }
}

/// The certificates `set_default_verify_paths` makes the context trust: those of the `SSL_CERT_FILE` file and of the
/// `SSL_CERT_DIR` directories when either variable is set, otherwise the operating system roots. Both are read when the
/// context is built, as OpenSSL reads the file then.
fn default_verify_certificates(environment: &dyn EnvironmentVariable) -> Vec<CertificateDer<'static>> {
    let file = environment.var_os(OsStr::new("SSL_CERT_FILE")).map(PathBuf::from);
    let directories: Vec<PathBuf> = environment
        .var_os(OsStr::new("SSL_CERT_DIR"))
        .map(|directories| std::env::split_paths(&directories).collect())
        .unwrap_or_default();
    if file.is_none() && directories.is_empty() {
        return rustls_native_certs::load_native_certs().certs;
    }
    let mut certificates = rustls_native_certs::load_certs_from_paths(file.as_deref(), None).certs;
    for directory in &directories {
        certificates.extend(rustls_native_certs::load_certs_from_paths(None, Some(directory.as_path())).certs);
    }
    certificates
}

/// An open TLS stream. A peer that closes without `close_notify` ends the body, as `suppress_ragged_eofs` does.
struct TlsStream(StreamOwned<ClientConnection, Gate<TcpStream>>);

impl Read for TlsStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buffer) {
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
            other => other,
        }
    }
}

impl Write for TlsStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// The default HTTPS context `urlopen` uses.
#[derive(Debug)]
pub(crate) struct UrlopenTls {
    config: Arc<ClientConfig>,
}

impl UrlopenTls {
    /// The context trusting `trusted` (DER certificates), or the default verify paths of the process environment
    /// when `None`.
    #[cfg(test)]
    pub(crate) fn new(trusted: Option<&[Vec<u8>]>) -> Result<Self, RemoteSchemaError> {
        Self::from_environment(trusted, &crate::adapters::SystemEnvironment)
    }

    /// The context trusting `trusted`, or the default verify paths `environment` names when `None`.
    pub(crate) fn from_environment(
        trusted: Option<&[Vec<u8>]>,
        environment: &dyn EnvironmentVariable,
    ) -> Result<Self, RemoteSchemaError> {
        let mut provider = rustls::crypto::aws_lc_rs::default_provider();
        provider.kx_groups = vec![X25519MLKEM768, X25519, SECP256R1, SECP384R1];
        Self::with_provider(trusted, environment, provider)
    }

    /// The context over `provider`. Only a provider without cipher suites or groups, which a test builds, fails.
    fn with_provider(
        trusted: Option<&[Vec<u8>]>,
        environment: &dyn EnvironmentVariable,
        provider: CryptoProvider,
    ) -> Result<Self, RemoteSchemaError> {
        let provider = Arc::new(provider);
        let mut roots = RootCertStore::empty();
        match trusted {
            Some(certificates) => {
                roots.add_parsable_certificates(certificates.iter().map(|der| CertificateDer::from(der.as_slice())));
            }
            None => {
                roots.add_parsable_certificates(default_verify_certificates(environment));
            }
        }
        let inner = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider))
            .build()
            .ok();
        let verifier = Arc::new(OpenSslReasons {
            inner,
            provider: Arc::clone(&provider),
        });
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| url_error(&format!("[SSL] {error} {SSL_LOCATION}")))?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        config.resumption = Resumption::disabled().tls12_resumption(Tls12Resumption::SessionIdOrTickets);
        Ok(Self {
            config: Arc::new(config),
        })
    }

    /// `context.wrap_socket(sock, server_hostname=host)` with the handshake done. A host `rustls` cannot name is
    /// reported as the mismatch OpenSSL finds for it after the handshake.
    pub(crate) fn connect(&self, stream: TcpStream, host: &str) -> Result<Box<dyn Duplex>, RemoteSchemaError> {
        let name = ServerName::try_from(host.to_owned()).map_err(|_| {
            url_error(&format!(
                "[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: Hostname mismatch, certificate is not \
                 valid for '{host}'. {SSL_LOCATION}"
            ))
        })?;
        let dns = matches!(name, ServerName::DnsName(_));
        let connection = ClientConnection::new(Arc::clone(&self.config), name)
            .map_err(|error| url_error(&format!("[SSL] {error} {SSL_LOCATION}")))?;
        let mut tls = StreamOwned::new(connection, Gate::new(stream, dns));
        while tls.conn.is_handshaking() {
            tls.conn
                .complete_io(&mut tls.sock)
                .map_err(|error| handshake_failure(&error))?;
        }
        tls.sock.step_aside();
        Ok(Box::new(TlsStream(tls)))
    }
}

#[cfg(test)]
#[path = "python_urlopen_tls_tests.rs"]
mod tests;
