use super::*;
use crate::held_writes::{HeldWrites, WAIT, expect_bytes};
use tokio::io::{AsyncWriteExt, duplex};

#[tokio::test]
async fn a_100_continue_is_flushed_to_the_client_before_its_body_is_awaited() -> Result<(), Box<dyn std::error::Error>>
{
    let (mut peer, client) = duplex(64 * 1024);
    let mut client = HeldWrites::new(client);
    let mut buffered = Vec::new();
    let continued = AtomicBool::new(false);
    let body = StreamedBody::new(&mut client, &mut buffered, Framing::ContentLength(5), true, &continued);
    let answer = async {
        expect_bytes(&mut peer, b"HTTP/1.1 100 Continue\r\n\r\n", "the 100 Continue").await;
        peer.write_all(b"hello").await
    };
    let (read, written) = tokio::join!(tokio::time::timeout(WAIT, body.read(16)), answer);
    written?;
    let bytes = read??.bytes;
    assert_eq!(bytes, b"hello");
    Ok(())
}

#[tokio::test]
async fn a_held_body_is_cut_at_the_size_asked_for() -> Result<(), Box<dyn std::error::Error>> {
    let body = RequestBody::new(b"abcdef", false);
    let read = body.read(4).await?;
    assert_eq!(read.bytes, b"abcd");
    assert!(read.cut);
    let read = body.read(6).await?;
    assert_eq!(read.bytes, b"abcdef");
    assert!(!read.cut);
    Ok(())
}

#[tokio::test]
async fn a_held_chunked_body_is_decoded_and_a_malformed_one_is_unavailable() -> Result<(), Box<dyn std::error::Error>> {
    let body = RequestBody::new(b"3\r\nabc\r\n0\r\n\r\n", true);
    assert_eq!(body.read(10).await?.bytes, b"abc");
    let broken = RequestBody::new(b"zz\r\n", true);
    assert_eq!(broken.read(10).await, Err(BodyUnavailableError::Malformed));
    Ok(())
}

#[test]
fn a_sized_body_waits_for_its_bytes_and_a_long_one_is_cut() -> Result<(), Box<dyn std::error::Error>> {
    let sized = Framing::ContentLength(6);
    assert!(matches!(scan_body(b"abc", sized, 10)?, Scan::Need));
    assert!(matches!(scan_body(b"abcdefXX", sized, 10)?, Scan::Done(bytes) if bytes == b"abcdef"));
    assert!(matches!(scan_body(b"abcdefXX", sized, 4)?, Scan::Cut(bytes) if bytes == b"abcd"));
    Ok(())
}

#[test]
fn a_chunked_body_is_complete_only_after_its_last_chunk_and_trailers() -> Result<(), Box<dyn std::error::Error>> {
    let chunked = Framing::Chunked;
    assert!(matches!(scan_body(b"3\r\nabc\r\n", chunked, 10)?, Scan::Need));
    assert!(matches!(scan_body(b"3\r\nabc\r\n0\r\n", chunked, 10)?, Scan::Need));
    assert!(
        matches!(scan_body(b"3\r\nabc\r\n2\r\nde\r\n0\r\nx: y\r\n\r\n", chunked, 10)?, Scan::Done(bytes) if bytes == b"abcde")
    );
    assert!(matches!(scan_body(b"3\r\nabc\r\n2\r\nde\r\n", chunked, 4)?, Scan::Cut(bytes) if bytes == b"abcd"));
    assert!(matches!(
        scan_body(b"3\r\nabcXX", chunked, 10),
        Err(BodyUnavailableError::Malformed)
    ));
    Ok(())
}

#[tokio::test]
async fn a_streamed_body_is_pulled_from_the_client_and_stays_for_forwarding() -> Result<(), Box<dyn std::error::Error>>
{
    let (mut client, mut peer) = tokio::io::duplex(64);
    let mut buffered = b"ab".to_vec();
    let continued = AtomicBool::new(false);
    let source = StreamedBody::new(&mut client, &mut buffered, Framing::ContentLength(5), true, &continued);
    tokio::spawn(async move {
        let mut head = [0u8; 25];
        peer.read_exact(&mut head).await?;
        peer.write_all(b"cde").await?;
        Ok::<_, std::io::Error>(peer)
    });
    let read = RequestBody::streamed(&source).read(100).await?;
    assert_eq!(read.bytes, b"abcde");
    assert!(!read.cut);
    assert_eq!(buffered, b"abcde");
    assert!(continued.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test]
async fn a_reader_is_served_while_the_callback_decides() -> Result<(), Box<dyn std::error::Error>> {
    let body = RequestBody::new(b"hello", false);
    let seen = serving_reads(Some(body), |reader| async move { reader.read(3).await }).await?;
    assert_eq!(seen.bytes, b"hel");
    let none = serving_reads(None, |reader| async move { reader.read(3).await }).await;
    assert_eq!(none, Err(BodyUnavailableError::Absent));
    Ok(())
}

#[test]
fn a_body_with_no_framing_is_empty_and_a_short_chunk_waits() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(scan_body(b"", Framing::None, 10)?, Scan::Done(bytes) if bytes.is_empty()));
    assert!(matches!(scan_body(b"6\r\nabc", Framing::Chunked, 10)?, Scan::Need));
    assert!(matches!(scan_body(b"8\r\nabc", Framing::Chunked, 4)?, Scan::Need));
    Ok(())
}

#[test]
fn a_request_body_debug_hides_its_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let body = RequestBody::new(b"secret", false);
    let shown = format!("{body:?}");
    assert!(shown.starts_with("RequestBody"));
    assert!(!shown.contains("secret"));
    Ok(())
}
