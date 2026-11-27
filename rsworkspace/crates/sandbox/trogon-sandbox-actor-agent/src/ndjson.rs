use axum::body::Body;
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::response::Response;
use bytes::Bytes;
use serde::Serialize;
use tracing::error;

/// One newline-terminated JSON line.
///
/// A frame that cannot be encoded degrades to an empty line, which a reader skips, rather than ending the stream.
pub(crate) fn encode_line<T: Serialize>(frame: &T) -> Bytes {
    let mut line = serde_json::to_vec(frame)
        .inspect_err(|error| error!(%error, "failed to encode a frame"))
        .unwrap_or_default();
    line.push(b'\n');
    Bytes::from(line)
}

fn ndjson_response(body: Body) -> Response {
    let mut response = Response::new(body);
    let content_type = HeaderValue::from_static("application/x-ndjson; charset=utf-8");
    response.headers_mut().insert(CONTENT_TYPE, content_type);
    response
}

/// A response whose whole body is the one frame.
pub(crate) fn single_frame<T: Serialize>(frame: &T) -> Response {
    ndjson_response(Body::from(encode_line(frame)))
}

/// A response whose body is the lines of `lines`, written as they are produced.
pub(crate) fn ndjson_stream<S>(lines: S) -> Response
where
    S: futures::Stream<Item = Result<Bytes, std::convert::Infallible>> + Send + 'static,
{
    ndjson_response(Body::from_stream(lines))
}
