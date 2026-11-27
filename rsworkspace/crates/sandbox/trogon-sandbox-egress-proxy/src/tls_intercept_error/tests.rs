use super::*;

#[test]
fn upstream_connect_and_handshake_failures_keep_their_own_kind() {
    let connect = TlsInterceptError::from(HttpInterceptError::UpstreamConnect(io::ErrorKind::TimedOut.into()));
    assert!(matches!(connect, TlsInterceptError::UpstreamConnect(_)));
    let handshake = TlsInterceptError::from(HttpInterceptError::UpstreamHandshake(io::ErrorKind::InvalidData.into()));
    assert!(matches!(handshake, TlsInterceptError::UpstreamHandshake(_)));
}
