/// The trust-store environment variable of each common TLS client.
///
/// Several tools ignore `SSL_CERT_FILE`, and some are pre-set to a system bundle in the parent environment, so each
/// one is set explicitly.
pub const CA_TRUST_ENV_VARS: [&str; 11] = [
    "SSL_CERT_FILE",
    "NODE_EXTRA_CA_CERTS",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    "PIP_CERT",
    "AWS_CA_BUNDLE",
    "CARGO_HTTP_CAINFO",
    "DENO_CERT",
    "CLOUDSDK_CORE_CUSTOM_CA_CERTS_FILE",
    "NIX_SSL_CERT_FILE",
];

/// Every trust-store variable paired with `ca_file`, a path as the workload sees it.
pub fn ca_trust_env(ca_file: &str) -> Vec<(&'static str, String)> {
    CA_TRUST_ENV_VARS
        .iter()
        .map(|name| (*name, ca_file.to_string()))
        .collect()
}
