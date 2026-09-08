use super::*;

/// A panicking client proxy must not read as a clean disconnect.
#[tokio::test]
async fn a_panicked_client_proxy_becomes_an_error() {
    let join_error = tokio::spawn(async { panic!("client proxy exploded") })
        .await
        .expect_err("the task panicked, so joining must fail");
    assert!(join_error.is_panic());

    let outcome = client_task_outcome(Err(join_error));

    let error = outcome.expect_err("a JoinError must surface as a connection error");
    assert_eq!(
        error.code,
        agent_client_protocol::ErrorCode::InternalError,
        "the SDK transport should see an internal error, not a clean close"
    );
}

#[tokio::test]
async fn a_clean_client_proxy_exit_stays_ok() {
    assert!(client_task_outcome(Ok(())).is_ok());
}

/// A boundary error must not be reported to the transport as a clean close.
#[test]
fn a_boundary_error_stays_an_error() {
    let error = agent_client_protocol::Error::internal_error();
    let outcome = connection_outcome(Err(error));

    assert_eq!(
        outcome.expect_err("a boundary error must propagate").code,
        agent_client_protocol::ErrorCode::InternalError
    );
}

/// Both exit variants are ordinary closes, including a peer that hung up.
#[test]
fn both_boundary_exits_are_clean_closes() {
    assert!(connection_outcome(Ok(BoundaryExit::Main(()))).is_ok());
    assert!(connection_outcome(Ok(BoundaryExit::TransportClosed)).is_ok());
}
