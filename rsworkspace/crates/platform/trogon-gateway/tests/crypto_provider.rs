//! Guards the dependency graph that made every TLS handshake panic at runtime.
//!
//! `trogon-gateway` reaches rustls through both provider features, `aws-lc-rs`
//! and `ring` (the latter through reqwest 0.12), so rustls refuses to pick one
//! and anything building a TLS client without an installed provider panics
//! instead of connecting. This fails if the explicit provider install stops
//! happening while the graph still enables both.

#[test]
fn tls_clients_build_once_the_provider_is_installed() {
    trogon_std::tls::install_default_crypto_provider();

    let _ = rustls::ClientConfig::builder();
}
