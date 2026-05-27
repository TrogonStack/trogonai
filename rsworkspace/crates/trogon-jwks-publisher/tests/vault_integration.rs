//! Live Vault Transit test — requires a running Vault with a configured transit key.

#[cfg(feature = "vault")]
#[tokio::test]
#[ignore = "requires live Vault at TROGON_JWKS_VAULT_ADDR with transit key configured"]
async fn vault_transit_sign_and_publish_jwks() {
    use trogon_jwks_publisher::keys::vault::VaultKeySource;
    use trogon_jwks_publisher::keys::KeySource;

    let addr = std::env::var("TROGON_JWKS_VAULT_ADDR").unwrap_or_else(|_| "http://127.0.0.1:8200".into());
    let key = std::env::var("TROGON_JWKS_VAULT_TRANSIT_KEY").expect("TROGON_JWKS_VAULT_TRANSIT_KEY");
    let token = std::env::var("TROGON_JWKS_VAULT_TOKEN").expect("TROGON_JWKS_VAULT_TOKEN");
    let mount = std::env::var("TROGON_JWKS_VAULT_TRANSIT_MOUNT").unwrap_or_else(|_| "transit".into());

    let source = VaultKeySource::new(addr, mount, key, token, None)
        .await
        .expect("vault source");
    let jwks = source.current().await.expect("jwks");
    assert!(!jwks.keys.is_empty());
}
