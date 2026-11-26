use axum::http::StatusCode;

use super::*;

#[test]
fn encode_failure_maps_to_internal_server_error() {
    let response = metrics_response(Err(ObservabilityError::Metrics(prometheus::Error::Msg(
        "boom".to_owned(),
    ))));
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[test]
fn success_maps_to_text_plain() {
    let response = metrics_response(Ok("x 1\n".to_owned()));
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).map(|v| v.as_bytes()),
        Some(METRICS_CONTENT_TYPE.as_bytes())
    );
}

#[test]
fn listener_error_carries_address_and_source() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = "127.0.0.1:1".parse()?;
    let error = listener_error(addr)(std::io::Error::other("boom"));
    assert!(matches!(error, ObservabilityError::Listener { addr: failed, .. } if failed == addr));
    Ok(())
}

#[test]
fn serve_outcome_maps_io_errors_to_listener_errors() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = "127.0.0.1:1".parse()?;
    assert!(served(addr, Ok(())).is_ok());
    assert!(matches!(
        served(addr, Err(std::io::Error::other("boom"))),
        Err(ObservabilityError::Listener { addr: failed, .. }) if failed == addr
    ));
    Ok(())
}

#[test]
fn exit_is_logged_only_for_errors() -> Result<(), Box<dyn std::error::Error>> {
    let _subscriber = tracing::subscriber::set_default(tracing_subscriber::fmt().with_writer(std::io::sink).finish());
    let addr: SocketAddr = "127.0.0.1:1".parse()?;
    log_exit(addr, Ok(()));
    log_exit(
        addr,
        Err(ObservabilityError::Metrics(prometheus::Error::Msg("boom".to_owned()))),
    );
    Ok(())
}
