//! Guards the dependency graph that made every TLS handshake panic at runtime.
//!
//! `trogon-gateway` reaches rustls through both provider features: `aws-lc-rs`
//! via the OTLP exporter and `ring` via async-nats and twilight. rustls refuses
//! to pick one, so anything building a TLS client panicked instead of
//! connecting. This fails if the explicit provider install stops happening or a
//! dependency change reintroduces the ambiguity.

#[test]
fn tls_clients_build_once_the_provider_is_installed() {
    assert!(trogon_std::tls::install_default_crypto_provider().is_ok());

    let _ = rustls::ClientConfig::builder();
}
