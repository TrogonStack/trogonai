use super::*;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn shutdown_delivers_what_was_held_then_closes() -> Result<(), Box<dyn std::error::Error>> {
    let (near, mut far) = tokio::io::duplex(64);
    let mut held = HeldWrites::new(near);
    held.write_all(b"held").await?;

    held.shutdown().await?;

    let mut received = Vec::new();
    far.read_to_end(&mut received).await?;
    assert_eq!(received, b"held");
    Ok(())
}
