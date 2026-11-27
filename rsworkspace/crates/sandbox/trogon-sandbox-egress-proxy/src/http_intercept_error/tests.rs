use super::*;

#[test]
fn only_io_errors_that_mean_a_gone_upstream_end_a_reused_stream_silently() -> Result<(), Box<dyn std::error::Error>> {
    let reset = HttpInterceptError::UpstreamWrite(std::io::ErrorKind::BrokenPipe.into());
    assert!(reset.ends_silently(StreamUse::Reused));
    assert!(!reset.ends_silently(StreamUse::Fresh));
    let other = HttpInterceptError::UpstreamRead(std::io::ErrorKind::InvalidData.into());
    assert!(!other.ends_silently(StreamUse::Reused));
    let unrelated = HttpInterceptError::MalformedRequest("x".into());
    assert!(!unrelated.ends_silently(StreamUse::Reused));
    Ok(())
}
