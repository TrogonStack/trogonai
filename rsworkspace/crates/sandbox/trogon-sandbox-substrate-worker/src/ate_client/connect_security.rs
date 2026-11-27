use super::{BearerTokenFile, TlsTrust};

/// TLS and authentication for the connection to `ate-api-server`. Both are optional so a
/// plaintext, unauthenticated endpoint (a local test double) still connects.
#[derive(Debug, Clone, Default)]
pub struct ConnectSecurity {
    pub tls: Option<TlsTrust>,
    pub bearer_token_file: Option<BearerTokenFile>,
}
