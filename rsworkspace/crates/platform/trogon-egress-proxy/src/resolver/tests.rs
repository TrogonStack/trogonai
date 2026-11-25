use super::*;

#[tokio::test]
async fn localhost_resolves_to_loopback_addresses() {
    let host = HostName::new("localhost").expect("host");
    let addresses = SystemResolver.resolve(&host).await.expect("resolve");
    assert!(!addresses.is_empty());
    assert!(addresses.iter().all(IpAddr::is_loopback));
}

#[tokio::test]
async fn a_name_that_cannot_exist_fails() {
    let host = HostName::new("nonexistent.invalid").expect("host");
    assert!(SystemResolver.resolve(&host).await.is_err());
}
