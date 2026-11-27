/// Why router TLS could not be configured or established.
#[derive(Debug, thiserror::Error)]
pub enum RouterTlsError {
    #[error("router TLS requires CA, client certificate and client private key file paths")]
    MissingPath,
    #[error("router TLS requires a valid explicit server name")]
    InvalidServerName,
    #[error("loading router TLS files: {0}")]
    LoadTask(#[source] tokio::task::JoinError),
    #[error("opening router TLS {description} file: {source}")]
    Open {
        description: String,
        #[source]
        source: std::io::Error,
    },
    #[error("reading router TLS {description} file: {source}")]
    Read {
        description: String,
        #[source]
        source: std::io::Error,
    },
    #[error("router TLS {description} exceeds the PEM size limit")]
    PemTooLarge { description: String },
    #[error("invalid router TLS {description} PEM")]
    InvalidPem { description: String },
    #[error("router TLS {description} contains no certificates")]
    NoCertificates { description: String },
    #[error("invalid router TLS CA certificate")]
    InvalidCaCertificate,
    #[error("invalid router TLS client private key PEM")]
    InvalidKeyPem,
    #[error("router TLS client identity has no private key")]
    MissingPrivateKey,
    #[error("configuring router TLS protocol versions: {0}")]
    ProtocolVersions(#[source] tokio_rustls::rustls::Error),
    #[error("invalid or mismatched router TLS client certificate and key")]
    InvalidClientIdentity,
    #[error("connecting to router CONNECT-TLS listener: {0}")]
    Connect(#[source] std::io::Error),
    #[error("TLS handshake with router CONNECT listener: {0}")]
    Handshake(#[source] std::io::Error),
}
