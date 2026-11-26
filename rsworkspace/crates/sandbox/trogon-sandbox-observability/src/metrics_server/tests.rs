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
