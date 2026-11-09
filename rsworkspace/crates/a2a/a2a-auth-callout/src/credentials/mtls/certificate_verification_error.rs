#[derive(Debug, thiserror::Error)]
pub enum CertificateVerificationError {
    #[error("verified certificate path metadata is unavailable")]
    PathMetadata,
    #[error("client certificate path validation failed")]
    Path(#[from] webpki::Error),
    #[error("client certificate extension is invalid")]
    Extension(#[from] x509_parser::error::X509Error),
    #[error("client certificate is not permitted to sign")]
    LeafKeyUsage,
    #[error("certificate issuer is not permitted to sign certificates")]
    IssuerKeyUsage,
    #[error("client certificate is not permitted for client authentication")]
    ClientAuthUsage,
    #[error("certificate verification time predates the Unix epoch")]
    VerificationTime(#[from] std::num::TryFromIntError),
}
