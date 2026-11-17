use super::*;

#[test]
fn production_identity_refuses_tcp_even_when_sdk_accepts_it() {
    assert!(UnixEndpoint::new("tcp://127.0.0.1:8081").is_err());
    assert!(UnixEndpoint::new("unix:relative.sock").is_err());
    assert!(UnixEndpoint::new("unix:///run/identity/api.sock").is_ok());
}
