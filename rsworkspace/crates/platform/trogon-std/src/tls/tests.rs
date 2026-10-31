use std::sync::Arc;

use super::install_default_crypto_provider;
use rustls::crypto::CryptoProvider;

#[test]
fn installs_a_process_wide_provider() {
    install_default_crypto_provider();

    assert!(CryptoProvider::get_default().is_some());
}

#[test]
fn installing_again_keeps_the_installed_provider() {
    install_default_crypto_provider();
    let installed = CryptoProvider::get_default().map(Arc::as_ptr);

    install_default_crypto_provider();

    assert_eq!(CryptoProvider::get_default().map(Arc::as_ptr), installed);
}
