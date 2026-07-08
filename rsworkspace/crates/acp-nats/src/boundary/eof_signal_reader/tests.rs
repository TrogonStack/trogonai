use super::*;
use futures::AsyncReadExt;

#[tokio::test]
async fn fires_signal_on_eof() {
    let (mut reader, mut rx) = EofSignalReader::new(futures::io::Cursor::new(b"ab".to_vec()));

    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();

    assert_eq!(buf, b"ab");
    assert!(rx.try_recv().is_ok());
}

#[tokio::test]
async fn no_signal_before_eof() {
    let (mut reader, mut rx) = EofSignalReader::new(futures::io::Cursor::new(b"ab".to_vec()));

    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf).await.unwrap();

    assert!(rx.try_recv().is_err());
}
