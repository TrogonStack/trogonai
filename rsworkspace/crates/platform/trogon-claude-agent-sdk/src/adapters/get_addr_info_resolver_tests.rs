use std::error::Error as StdError;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::mpsc;
use std::vec;

use reqwest::dns::{Name, Resolve};
use tokio::task::JoinError;

use super::{GetAddrInfoResolver, Lookup, NameLookupError};

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

#[tokio::test]
async fn a_lookup_that_panics_resumes_the_panic_in_the_caller() -> TestResult {
    let lookup = Lookup(tokio::task::spawn_blocking(
        || -> io::Result<vec::IntoIter<SocketAddr>> { std::panic::resume_unwind(Box::new("lookup panicked")) },
    ));
    let failure = tokio::spawn(lookup.answer()).await.err().ok_or("expected the panic")?;
    let payload = failure
        .try_into_panic()
        .map_err(|_| "expected a panic, not a cancellation")?;
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"lookup panicked"));
    Ok(())
}

#[test]
fn a_cancelled_lookup_is_an_interrupted_error() -> TestResult {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()?;
    let failure = runtime.block_on(async {
        let (release, wait) = mpsc::channel::<()>();
        let busy = tokio::task::spawn_blocking(move || wait.recv());
        let queued = tokio::task::spawn_blocking(|| -> io::Result<vec::IntoIter<SocketAddr>> {
            Err(io::Error::other("the cancelled lookup ran"))
        });
        queued.abort();
        release.send(())?;
        busy.await??;
        Ok::<_, Box<dyn StdError + Send + Sync>>(Lookup(queued).answer().await.err())
    })?;
    let failure = failure.ok_or("expected the cancellation")?;
    assert_eq!(failure.kind(), io::ErrorKind::Interrupted);
    assert!(
        failure
            .get_ref()
            .and_then(|source| source.downcast_ref::<JoinError>())
            .is_some_and(JoinError::is_cancelled)
    );
    Ok(())
}
