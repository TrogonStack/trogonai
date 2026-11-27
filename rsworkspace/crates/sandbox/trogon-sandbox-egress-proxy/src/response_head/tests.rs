use super::*;

#[tokio::test]
async fn a_response_head_without_an_end_past_the_limit_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let mut buf = vec![b'a'; MAX_HEAD_BYTES + 1];
    let mut stream: &[u8] = b"";
    let result = read_response_header_end(&mut stream, &mut buf).await;
    assert!(matches!(result, Err(HttpInterceptError::MalformedRequest(_))));
    Ok(())
}
