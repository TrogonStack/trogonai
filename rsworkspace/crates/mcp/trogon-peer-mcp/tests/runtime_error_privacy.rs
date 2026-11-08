use std::error::Error as _;
use trogon_peer_mcp::RuntimeError;

#[test]
fn runtime_diagnostics_hide_backend_details_but_preserve_typed_causes() {
    let marker = "synthetic credential source detail that must stay out of runtime diagnostics";
    let failure = RuntimeError::Connect(trogon_nats::ConnectError::InvalidCredentials(std::io::Error::other(
        marker,
    )));
    assert!(
        !format!("{failure:?}").contains(marker),
        "runtime Debug leaked a backend source"
    );
    assert!(
        !failure.to_string().contains(marker),
        "public diagnostic leaked a backend source"
    );
    assert!(failure.source().is_some(), "redaction must not erase the typed cause");
}
