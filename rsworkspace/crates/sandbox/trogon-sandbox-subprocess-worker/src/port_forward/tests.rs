use super::*;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Dials the port on the worker's own loopback, standing in for a lease.
struct Loopback;

#[async_trait]
impl Connector for Loopback {
    async fn connect(&self, port: u16) -> io::Result<Box<dyn BridgeIo>> {
        Ok(Box::new(TcpStream::connect(("127.0.0.1", port)).await?))
    }
}

fn lease(id: &str) -> LeaseId {
    LeaseId::new(id)
}

fn endpoint(response: GetSandboxDomainResponse) -> Result<String, String> {
    match response {
        GetSandboxDomainResponse::Available { endpoint, .. } => Ok(endpoint),
        other => Err(format!("expected Available, got {other:?}")),
    }
}

async fn registry_with_loopback(id: &str) -> PortForwardRegistry {
    let registry = PortForwardRegistry::new();
    registry
        .register(&lease(id), Reach::Connector(Arc::new(Loopback)))
        .await;
    registry
}

async fn echo_server() -> io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let _ = stream.read_to_end(&mut buf).await;
                let _ = stream.write_all(&buf).await;
            });
        }
    });
    Ok(port)
}

#[tokio::test]
async fn a_forward_carries_bytes_both_ways_and_honours_half_close() -> TestResult {
    let port = echo_server().await?;
    let registry = registry_with_loopback("l1").await;

    let endpoint = endpoint(registry.resolve(&lease("l1"), port).await)?;
    let mut client = TcpStream::connect(&endpoint).await?;
    client.write_all(b"ping").await?;
    client.shutdown().await?;
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut reply))
        .await
        .map_err(|_| "the reply never finished")??;

    assert_eq!(reply, b"ping");
    Ok(())
}

#[tokio::test]
async fn the_server_closing_first_ends_the_client_connection() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream.write_all(b"bye").await;
        }
    });
    let registry = registry_with_loopback("l1").await;

    let endpoint = endpoint(registry.resolve(&lease("l1"), port).await)?;
    let mut client = TcpStream::connect(&endpoint).await?;
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut reply))
        .await
        .map_err(|_| "the client was left hanging")??;

    assert_eq!(reply, b"bye");
    Ok(())
}

#[tokio::test]
async fn resolving_the_same_port_twice_reuses_the_listener() -> TestResult {
    let registry = registry_with_loopback("l1").await;

    let first = endpoint(registry.resolve(&lease("l1"), 8080).await)?;
    let second = endpoint(registry.resolve(&lease("l1"), 8080).await)?;

    assert_eq!(first, second);
    Ok(())
}

#[tokio::test]
async fn an_unknown_lease_is_not_found() -> TestResult {
    let registry = PortForwardRegistry::new();

    let response = registry.resolve(&lease("gone"), 8080).await;

    assert_eq!(
        response,
        GetSandboxDomainResponse::Denied(LeaseError::LeaseNotFound {
            lease_id: lease("gone")
        })
    );
    assert!(!registry.holds(&lease("gone")).await);
    Ok(())
}

#[tokio::test]
async fn port_zero_is_not_exposed() -> TestResult {
    let registry = registry_with_loopback("l1").await;

    assert_eq!(
        registry.resolve(&lease("l1"), 0).await,
        GetSandboxDomainResponse::PortNotExposed
    );
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_egress_relay_ports_are_never_exposed() -> TestResult {
    let registry = registry_with_loopback("l1").await;

    for port in reserved_ports() {
        assert_eq!(
            registry.resolve(&lease("l1"), port).await,
            GetSandboxDomainResponse::PortNotExposed
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_lease_is_capped_at_its_forward_limit() -> TestResult {
    let registry = registry_with_loopback("l1").await;
    for port in 0..MAX_FORWARDS_PER_LEASE as u16 {
        endpoint(registry.resolve(&lease("l1"), 9000 + port).await)?;
    }

    assert_eq!(
        registry.resolve(&lease("l1"), 9100).await,
        GetSandboxDomainResponse::PortNotExposed
    );
    // A port already forwarded still resolves at the cap.
    endpoint(registry.resolve(&lease("l1"), 9000).await)?;
    Ok(())
}

#[tokio::test]
async fn a_lease_without_a_namespace_of_its_own_is_unsupported() -> TestResult {
    let registry = PortForwardRegistry::new();
    registry.register(&lease("l1"), Reach::Unsupported).await;

    assert_eq!(
        registry.resolve(&lease("l1"), 8080).await,
        GetSandboxDomainResponse::Unsupported
    );
    assert!(registry.holds(&lease("l1")).await);
    Ok(())
}

#[tokio::test]
async fn releasing_a_lease_closes_its_listener_and_open_bridges() -> TestResult {
    let port = echo_server().await?;
    let registry = registry_with_loopback("l1").await;
    let endpoint = endpoint(registry.resolve(&lease("l1"), port).await)?;
    // Held open: the echo server only answers at end of input.
    let mut open = TcpStream::connect(&endpoint).await?;
    open.write_all(b"x").await?;

    registry.remove(&lease("l1")).await;

    let mut buf = Vec::new();
    let ended = tokio::time::timeout(Duration::from_secs(5), open.read_to_end(&mut buf)).await;
    assert!(ended.is_ok(), "the open bridge outlived the lease: {ended:?}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let rejected = TcpStream::connect(&endpoint).await;
    assert!(rejected.is_err(), "the listener outlived the lease");
    assert_eq!(
        registry.resolve(&lease("l1"), port).await,
        GetSandboxDomainResponse::Denied(LeaseError::LeaseNotFound { lease_id: lease("l1") })
    );
    Ok(())
}

#[tokio::test]
async fn a_listener_cannot_be_added_to_a_lease_released_while_resolving() -> TestResult {
    let registry = registry_with_loopback("l1").await;
    let resolving = registry.clone();
    let release = registry.clone();

    let (resolved, _) = tokio::join!(async move { resolving.resolve(&lease("l1"), 8080).await }, async move {
        release.remove(&lease("l1")).await
    },);

    // Whichever ran first, no forward outlives the release.
    assert!(!registry.holds(&lease("l1")).await);
    if let GetSandboxDomainResponse::Available { endpoint, .. } = resolved {
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(TcpStream::connect(&endpoint).await.is_err());
    }
    Ok(())
}

struct Refuses(io::ErrorKind);

#[async_trait]
impl Connector for Refuses {
    async fn connect(&self, _: u16) -> io::Result<Box<dyn BridgeIo>> {
        Err(self.0.into())
    }
}

#[tokio::test]
async fn a_revoked_registration_cannot_attach_to_a_reused_lease_id() -> TestResult {
    let registry = PortForwardRegistry::new();
    let old = registry
        .register_with_preview(&lease("reused"), Reach::Connector(Arc::new(Loopback)))
        .await
        .ok_or("a preview registration")?;
    registry.remove(&lease("reused")).await;
    let replacement = registry
        .register_with_preview(&lease("reused"), Reach::Connector(Arc::new(Loopback)))
        .await
        .ok_or("a preview registration")?;
    assert!(old.register_candidate(Arc::new(Loopback)).is_err());
    assert!(!replacement.is_revoked());
    let port = echo_server().await?;
    let mut connected = replacement.connect(port).await?;
    connected.write_all(b"replacement").await?;
    connected.shutdown().await?;
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), connected.read_to_end(&mut reply)).await??;
    registry.remove(&lease("reused")).await;
    assert_eq!(reply, b"replacement");
    Ok(())
}

#[tokio::test]
async fn retiring_a_terminal_membership_revokes_its_open_connection() -> TestResult {
    let preview = LeasePreview::new(Arc::new(Refuses(io::ErrorKind::ConnectionRefused)));
    let membership = preview.register_candidate(Arc::new(Loopback))?;
    let mut connected = preview.connect(echo_server().await?).await?;
    connected.write_all(b"held").await?;
    drop(membership);
    let mut byte = [0];
    let result = tokio::time::timeout(Duration::from_secs(1), connected.read(&mut byte)).await?;
    assert_eq!(
        result.err().ok_or("the read must fail")?.kind(),
        io::ErrorKind::ConnectionAborted
    );
    assert!(preview.connect(12345).await.is_err());
    Ok(())
}

#[tokio::test]
async fn an_unknown_namespace_failure_refuses_an_otherwise_available_route() -> TestResult {
    let preview = LeasePreview::new(Arc::new(Loopback));
    let _membership = preview.register_candidate(Arc::new(Refuses(io::ErrorKind::PermissionDenied)))?;
    let error = preview
        .connect(echo_server().await?)
        .await
        .err()
        .ok_or("unknown candidate must refuse the route")?;
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    Ok(())
}

#[test]
fn preview_namespace_memberships_are_bounded_by_the_terminal_limit() -> TestResult {
    let preview = LeasePreview::new(Arc::new(Loopback));
    let members: Vec<_> = (0..trogon_sandbox_protocol::MAX_TERMINALS_PER_LEASE)
        .map(|_| preview.register_candidate(Arc::new(Loopback)))
        .collect::<Result<_, _>>()?;
    assert!(preview.register_candidate(Arc::new(Loopback)).is_err());
    drop(members);
    assert!(preview.register_candidate(Arc::new(Loopback)).is_ok());
    Ok(())
}
