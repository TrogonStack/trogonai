use super::*;
#[cfg(unix)]
use std::os::fd::IntoRawFd;

#[test]
fn unix_prefix_selects_a_unix_socket() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        ListenAddress::parse("unix:/run/proxy.sock"),
        ListenAddress::Unix(PathBuf::from("/run/proxy.sock"))
    );
    assert_eq!(
        ListenAddress::parse("127.0.0.1:3128"),
        ListenAddress::Tcp("127.0.0.1:3128".to_string())
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn unix_listener_is_owner_only_and_accepts() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("listener-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("p.sock");
    let listener = Listener::bind(&ListenAddress::Unix(path.clone())).await?;
    let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    let client = tokio::spawn({
        let path = path.clone();
        async move { UnixStream::connect(path).await }
    });
    let (_, peer) = listener.accept().await?;
    assert_eq!(peer, PeerAddr::Unix);
    client.await??;
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn an_inherited_tcp_socket_is_adopted_and_accepts() -> Result<(), Box<dyn std::error::Error>> {
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let addr = std_listener.local_addr()?;
    let listener = Listener::from_inherited_fd(std_listener.into_raw_fd())?;
    assert!(matches!(listener, Listener::Tcp(_)));
    let client = tokio::spawn(async move { TcpStream::connect(addr).await });
    let (_, peer) = listener.accept().await?;
    assert!(matches!(peer, PeerAddr::Tcp(_)));
    client.await??;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn an_inherited_unix_socket_is_adopted_and_accepts() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("inherited-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("p.sock");
    let std_listener = std::os::unix::net::UnixListener::bind(&path)?;
    let listener = Listener::from_inherited_fd(std_listener.into_raw_fd())?;
    assert!(matches!(listener, Listener::Unix(_)));
    let client = tokio::spawn({
        let path = path.clone();
        async move { UnixStream::connect(path).await }
    });
    let (_, peer) = listener.accept().await?;
    assert_eq!(peer, PeerAddr::Unix);
    client.await??;
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn addresses_display_as_they_were_written() {
    assert_eq!(ListenAddress::Tcp("127.0.0.1:1".to_string()).to_string(), "127.0.0.1:1");
    assert_eq!(
        ListenAddress::Unix(PathBuf::from("/run/p.sock")).to_string(),
        "unix:/run/p.sock"
    );
}

#[test]
fn peers_display_as_an_address_or_unix() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(PeerAddr::Tcp("127.0.0.1:9".parse()?).to_string(), "127.0.0.1:9");
    #[cfg(unix)]
    assert_eq!(PeerAddr::Unix.to_string(), "unix");
    Ok(())
}

#[tokio::test]
async fn a_tcp_listener_binds_and_accepts() -> Result<(), Box<dyn std::error::Error>> {
    let listener = Listener::bind(&ListenAddress::Tcp("127.0.0.1:0".to_string())).await?;
    let addr = match &listener {
        Listener::Tcp(inner) => inner.local_addr()?,
        #[cfg(unix)]
        Listener::Unix(_) => return Err("expected a tcp listener".into()),
    };
    let client = tokio::spawn(async move { TcpStream::connect(addr).await });
    let (_, peer) = listener.accept().await?;
    assert!(matches!(peer, PeerAddr::Tcp(_)));
    client.await??;
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_invalid_descriptor_is_not_adopted() {
    assert!(Listener::from_inherited_fd(-1).is_err());
}

#[cfg(unix)]
#[test]
fn a_descriptor_that_is_not_a_listening_socket_family_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    let family = libc::AF_NETLINK;
    #[cfg(not(target_os = "linux"))]
    let family = libc::AF_ROUTE;
    #[cfg(target_os = "linux")]
    let protocol = libc::NETLINK_ROUTE;
    #[cfg(not(target_os = "linux"))]
    let protocol = 0;
    let fd = unsafe { libc::socket(family, libc::SOCK_RAW, protocol) };
    if fd < 0 {
        return Err("could not open a raw socket".into());
    }
    let outcome = Listener::from_inherited_fd(fd);
    unsafe { libc::close(fd) };
    let error = outcome.err().ok_or("the descriptor was adopted")?;
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    Ok(())
}
