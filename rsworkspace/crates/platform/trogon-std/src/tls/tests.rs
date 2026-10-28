use super::install_default_crypto_provider;
use rustls::crypto::CryptoProvider;

#[test]
fn installs_a_process_wide_provider() {
    let _ = install_default_crypto_provider();

    assert!(CryptoProvider::get_default().is_some());
}

#[test]
fn refuses_to_replace_an_installed_provider() {
    let _ = install_default_crypto_provider();

    assert!(install_default_crypto_provider().is_err());
}
