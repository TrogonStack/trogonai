use tokio::io::AsyncWriteExt;

use super::LimitedWriter;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn writes_within_the_declared_size_pass_through() -> TestResult {
    let mut writer = LimitedWriter {
        inner: Vec::new(),
        remaining: 4,
    };
    writer.write_all(b"abcd").await?;
    writer.flush().await?;
    writer.shutdown().await?;
    assert_eq!(writer.inner, b"abcd");
    assert_eq!(writer.remaining, 0);
    Ok(())
}

#[tokio::test]
async fn writes_past_the_declared_size_fail() -> TestResult {
    let mut writer = LimitedWriter {
        inner: Vec::new(),
        remaining: 2,
    };
    let error = writer.write_all(b"abc").await.err().ok_or("write must fail")?;
    assert_eq!(error.kind(), std::io::ErrorKind::FileTooLarge);
    assert_eq!(writer.inner, b"ab");
    Ok(())
}
