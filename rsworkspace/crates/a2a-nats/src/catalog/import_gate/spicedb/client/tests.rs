//! Unit tests for the bulk-import permission-check trait + the
//! tonic-backed live client.
//!
//! End-to-end RPC behavior (real authzed/SpiceDB transport, bearer
//! auth round-trip) is exercised by the integration smoke harness
//! the runtime wiring PR carries -- this file pins the
//! construction failure modes and the trait shape so a regression
//! in either surfaces at unit-test speed.

use super::*;

#[tokio::test]
async fn connect_rejects_garbage_endpoint() {
    // Endpoints that don't parse as a tonic `Uri` must surface as
    // `Connect` at construction time rather than letting a
    // misformatted env value reach the first RPC and look like a
    // transport blip.
    let endpoint = SpiceDbEndpoint::parse("not a uri").expect("parse permits the shape");
    let token = SpiceDbToken::parse("secret").expect("non-empty token");
    let err = LiveBulkImportPermissionClient::connect(&endpoint, &token)
        .await
        .expect_err("garbage endpoint must error");
    assert!(matches!(err, SpiceDbImportGateBuildError::Connect(_)));
}

#[tokio::test]
async fn connect_rejects_token_with_invalid_metadata_chars() {
    // gRPC metadata is ASCII-only. A token containing a newline
    // can't go in an `Authorization` header without producing an
    // RPC-level error on every call, so reject at construction.
    let endpoint = SpiceDbEndpoint::parse("http://127.0.0.1:1").expect("uri-shaped");
    let token = SpiceDbToken::parse("bad\ntoken").expect("parse permits any non-empty");
    let err = LiveBulkImportPermissionClient::connect(&endpoint, &token)
        .await
        .expect_err("invalid metadata token must error");
    // Either Connect (channel dial fails first) or InvalidToken
    // (channel dial succeeds but metadata parse fails) is
    // acceptable -- the surface is one of the two precisely so
    // the failure can't pretend to be "everything's fine".
    assert!(
        matches!(
            err,
            SpiceDbImportGateBuildError::Connect(_) | SpiceDbImportGateBuildError::InvalidToken(_)
        ),
        "expected Connect or InvalidToken, got {err:?}",
    );
}

#[tokio::test]
async fn connect_against_unreachable_endpoint_errors() {
    // Smoke-cover the unreachable-endpoint path: port 1 reliably
    // refuses on every local sandbox. The error must classify as
    // Connect, never as Ok.
    let endpoint = SpiceDbEndpoint::parse("http://127.0.0.1:1").expect("uri-shaped");
    let token = SpiceDbToken::parse("secret").expect("non-empty token");
    let err = LiveBulkImportPermissionClient::connect(&endpoint, &token)
        .await
        .expect_err("unreachable endpoint must error");
    assert!(matches!(err, SpiceDbImportGateBuildError::Connect(_)));
}

#[test]
fn bulk_import_permission_check_is_object_safe() {
    // Object safety regression guard. The trait is consumed as
    // `Arc<dyn BulkImportPermissionCheck>` by the gateway's
    // Tier-1 gate, so a future signature change that breaks that
    // dyn-compatibility would fail this compile-time check.
    fn _accepts_dyn(_: &dyn BulkImportPermissionCheck) {}
}
