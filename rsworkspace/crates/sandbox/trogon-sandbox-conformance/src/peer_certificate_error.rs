/// Why a TLS probe's certificate digest was refused.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PeerCertificateError {
    #[error("the TLS peer certificate digest is not 64 lowercase hex digits")]
    NotSha256Hex,
}
