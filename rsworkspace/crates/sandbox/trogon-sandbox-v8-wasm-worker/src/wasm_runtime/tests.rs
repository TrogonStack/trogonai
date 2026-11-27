use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn ephemeral_bind_is_the_only_inbound_shape_permitted() -> TestResult {
    let wildcard: SocketAddr = "0.0.0.0:0".parse()?;
    let granted: SocketAddr = "127.0.0.1:8080".parse()?;
    let allowed = [granted];

    assert!(egress_use_allowed(&allowed, wildcard, SocketAddrUse::TcpBind));
    assert!(egress_use_allowed(&allowed, granted, SocketAddrUse::TcpConnect));
    for addr in [wildcard, granted] {
        assert!(!egress_use_allowed(&allowed, addr, SocketAddrUse::UdpBind));
        assert!(!egress_use_allowed(&allowed, addr, SocketAddrUse::UdpConnect));
        assert!(!egress_use_allowed(&allowed, addr, SocketAddrUse::UdpOutgoingDatagram));
    }
    assert!(!egress_use_allowed(&allowed, granted, SocketAddrUse::TcpBind));
    assert!(!egress_use_allowed(&allowed, wildcard, SocketAddrUse::TcpConnect));
    Ok(())
}

#[tokio::test]
async fn the_socket_check_admits_only_listed_connect_destinations() -> TestResult {
    let granted: SocketAddr = "127.0.0.1:8080".parse()?;
    let other: SocketAddr = "127.0.0.1:9090".parse()?;
    let check = egress_check(vec![granted]);

    assert!(check(granted, SocketAddrUse::TcpConnect).await);
    assert!(!check(other, SocketAddrUse::TcpConnect).await);
    Ok(())
}

#[test]
fn read_only_preopens_cannot_mutate() {
    assert_eq!(preopen_perms(MountAccess::ReadOnly), (DirPerms::READ, FilePerms::READ));
    assert_eq!(
        preopen_perms(MountAccess::ReadWrite),
        (DirPerms::all(), FilePerms::all())
    );
}
