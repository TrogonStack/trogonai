use super::*;

use bytes::Bytes;
use futures::stream;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn response(chunks: Vec<Result<&'static str, std::io::Error>>) -> Response {
    let parts: Vec<Result<Bytes, std::io::Error>> = chunks.into_iter().map(|chunk| chunk.map(Bytes::from)).collect();
    Response::from(http::Response::new(reqwest::Body::wrap_stream(stream::iter(parts))))
}

#[tokio::test]
async fn a_record_split_across_chunks_is_reassembled() -> TestResult {
    let mut collected = Collected::with_limit(1024);
    let body = response(vec![
        Ok("{\"stream\":\"stdout\",\"da"),
        Ok("ta\":\"hi\"}\n{\"command\":{\"id\":\"cmd_1\","),
        Ok("\"exitCode\":0}}"),
    ]);
    drain(body, &mut collected).await?;
    assert_eq!(collected.stdout.into_bytes(), b"hi");
    assert_eq!(collected.command_id.as_deref(), Some("cmd_1"));
    assert_eq!(collected.exit, Some(0));
    Ok(())
}

#[tokio::test]
async fn a_body_ending_on_a_newline_leaves_nothing_pending() -> TestResult {
    let mut collected = Collected::with_limit(1024);
    drain(response(vec![Ok("{\"command\":{\"id\":\"cmd_2\"}}\n")]), &mut collected).await?;
    assert_eq!(collected.command_id.as_deref(), Some("cmd_2"));
    assert_eq!(collected.exit, None);
    Ok(())
}

#[tokio::test]
async fn a_record_past_the_line_limit_is_refused() {
    let mut collected = Collected::with_limit(1024);
    let outcome = drain_within(response(vec![Ok("0123456789abcdef")]), &mut collected, 8).await;
    assert!(matches!(outcome, Err(VercelExecError::Capture(_))));
}

#[tokio::test]
async fn a_body_that_fails_midway_is_a_transport_error() {
    let mut collected = Collected::with_limit(1024);
    let body = response(vec![
        Ok("{\"command\":{\"id\":\"cmd_3\"}}\n"),
        Err(std::io::Error::other("connection reset")),
    ]);
    let outcome = drain(body, &mut collected).await;
    assert!(matches!(
        outcome,
        Err(VercelExecError::Api(VercelApiError::Transport(_)))
    ));
    assert_eq!(collected.command_id.as_deref(), Some("cmd_3"));
}
