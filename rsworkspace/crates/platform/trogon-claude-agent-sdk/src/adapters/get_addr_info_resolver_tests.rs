use std::error::Error as StdError;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use reqwest::dns::{Name, Resolve};

use super::{GetAddrInfoResolver, NameLookupError};

type TestResult = Result<(), Box<dyn StdError + Send + Sync>>;

async fn lookup(host: &str) -> Result<Vec<SocketAddr>, Box<dyn StdError + Send + Sync>> {
    let name: Name = host.parse()?;
    Ok(GetAddrInfoResolver.resolve(name).await?.collect())
}

#[tokio::test]
async fn an_ip_literal_is_answered_without_a_lookup() -> TestResult {
    assert_eq!(
        lookup("127.0.0.1").await?,
        vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)]
    );
    Ok(())
}

#[tokio::test]
async fn localhost_resolves_to_loopback_addresses_on_port_zero() -> TestResult {
    let addrs = lookup("localhost").await?;
    assert!(!addrs.is_empty());
    assert!(
        addrs.iter().all(|addr| addr.ip().is_loopback() && addr.port() == 0),
        "{addrs:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_name_that_cannot_exist_fails_as_a_name_lookup_error() -> TestResult {
    // RFC 6761 reserves `.invalid`: no resolver answers it, with or without a network.
    let failure = lookup("nonexistent.invalid").await.err().ok_or("expected a failure")?;
    let lookup_failure = failure
        .downcast_ref::<NameLookupError>()
        .ok_or("not a name lookup error")?;
    assert!(lookup_failure.source().is_some_and(|source| source.is::<io::Error>()));
    Ok(())
}

#[test]
fn a_custom_resolver_failure_keeps_its_io_error() {
    let failure = NameLookupError::new(io::Error::new(io::ErrorKind::NotFound, "no such host"));
    assert_eq!(failure.io_error().kind(), io::ErrorKind::NotFound);
}
