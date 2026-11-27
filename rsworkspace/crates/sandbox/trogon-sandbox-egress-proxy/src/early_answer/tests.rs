use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

#[tokio::test]
async fn a_body_write_passes_through_until_the_answer_is_in_and_is_dropped_after()
-> Result<(), Box<dyn std::error::Error>> {
    let answered = AtomicBool::new(false);
    let (writer, mut reader) = duplex(64);
    let mut body = UntilAnswered::new(writer, &answered);
    body.write_all(b"before").await?;
    assert_eq!(body.upstream_after(), UpstreamAfter::Reusable);
    answered.store(true, Ordering::SeqCst);
    body.write_all(&[0u8; 4096]).await?;
    assert_eq!(body.upstream_after(), UpstreamAfter::Spent);
    drop(body);
    let mut got = Vec::new();
    reader.read_to_end(&mut got).await?;
    assert_eq!(got, b"before");
    Ok(())
}

#[tokio::test]
async fn an_answer_after_the_whole_body_leaves_the_upstream_reusable() -> Result<(), Box<dyn std::error::Error>> {
    let answered = AtomicBool::new(false);
    let (writer, _reader) = duplex(64);
    let mut body = UntilAnswered::new(writer, &answered);
    body.write_all(b"all").await?;
    answered.store(true, Ordering::SeqCst);
    assert_eq!(body.upstream_after(), UpstreamAfter::Reusable);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_drained_client_that_goes_quiet_is_not_waited_for() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(64);
    client.write_all(b"x").await?;
    let mut quiet = QuietReads::new(&mut server);
    let mut one = [0u8; 1];
    quiet.read_exact(&mut one).await?;
    let started = Instant::now();
    let err = quiet.read(&mut one).await.err().ok_or("expected an error")?;
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    assert_eq!(started.elapsed(), DRAIN_QUIET);
    Ok(())
}

#[tokio::test]
async fn shutdown_reaches_the_upstream_before_the_answer_and_is_skipped_after() -> Result<(), Box<dyn std::error::Error>>
{
    let answered = AtomicBool::new(false);
    let (writer, mut reader) = duplex(64);
    let mut body = UntilAnswered::new(writer, &answered);
    body.shutdown().await?;
    let mut got = Vec::new();
    reader.read_to_end(&mut got).await?;
    assert!(got.is_empty());

    let answered = AtomicBool::new(true);
    let (writer, _reader) = duplex(64);
    let mut body = UntilAnswered::new(writer, &answered);
    body.shutdown().await?;
    body.flush().await?;
    assert_eq!(body.upstream_after(), UpstreamAfter::Reusable);
    Ok(())
}
