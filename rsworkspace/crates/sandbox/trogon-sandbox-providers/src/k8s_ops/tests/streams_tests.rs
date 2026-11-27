use crate::k8s_ops::{read_capped, read_tail};

#[tokio::test]
async fn read_capped_keeps_head_and_tail_of_a_long_stream() -> Result<(), Box<dyn std::error::Error>> {
    let data = vec![b'x'; 10_000];
    let (kept, truncation) = read_capped(&data[..], 100).await?;
    assert!(truncation.is_truncated());
    assert!(kept.len() < 200, "kept {} bytes", kept.len());

    let (kept, truncation) = read_capped(&b"short"[..], 100).await?;
    assert!(!truncation.is_truncated());
    assert_eq!(kept, b"short");
    Ok(())
}

#[tokio::test]
async fn read_tail_keeps_only_the_last_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let (kept, truncation) = read_tail(&b"0123456789"[..], 4).await?;
    assert!(truncation.is_truncated());
    assert_eq!(kept, b"6789");
    Ok(())
}

#[tokio::test]
async fn read_tail_keeps_a_short_stream_whole() -> Result<(), Box<dyn std::error::Error>> {
    let (kept, truncation) = read_tail(&b"abc"[..], 4).await?;
    assert!(!truncation.is_truncated());
    assert_eq!(kept, b"abc");
    Ok(())
}
