use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_contract::{Base64Data, TerminalOutput};

use super::Buffer;
use crate::constants::MAX_BUFFERED_BYTES;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn data(seq: u64, bytes: &[u8]) -> Result<TerminalOutput, Box<dyn std::error::Error>> {
    Ok(TerminalOutput::Data {
        seq,
        data: Base64Data::new(STANDARD.encode(bytes))?,
    })
}

fn exit(seq: u64, code: i32) -> TerminalOutput {
    TerminalOutput::Exit {
        seq,
        exit_code: Some(code),
    }
}

#[test]
fn a_read_drains_in_order_and_reports_the_exit_after_the_last_byte() -> TestResult {
    let mut buffer = Buffer::default();
    buffer.push(data(0, b"hello ")?);
    buffer.push(data(1, b"world")?);
    buffer.push(exit(2, 7));
    let first = buffer.take(6);
    assert_eq!(first.output, "hello ");
    assert!(!first.exited);
    let rest = buffer.take(100);
    assert_eq!(rest.output, "world");
    assert!(rest.exited);
    assert_eq!(rest.exit_code, Some(7));
    Ok(())
}

#[test]
fn a_full_buffer_drops_the_oldest_bytes_and_says_how_many() -> TestResult {
    let mut buffer = Buffer::default();
    buffer.push(data(0, &vec![b'a'; MAX_BUFFERED_BYTES])?);
    buffer.push(data(1, b"tail")?);
    let read = buffer.take(usize::MAX);
    assert_eq!(read.dropped_bytes, 4);
    assert!(read.output.ends_with("tail"));
    assert_eq!(read.output.len(), MAX_BUFFERED_BYTES);
    assert_eq!(buffer.take(10).dropped_bytes, 0);
    Ok(())
}

#[test]
fn a_gap_in_seq_counts_missed_frames() -> TestResult {
    let mut buffer = Buffer::default();
    buffer.push(data(0, b"a")?);
    buffer.push(data(3, b"b")?);
    assert_eq!(buffer.take(10).missed_frames, 2);
    Ok(())
}

#[test]
fn a_split_character_waits_for_its_remaining_bytes() -> TestResult {
    let mut buffer = Buffer::default();
    let snowman = "\u{2603}".as_bytes();
    let mut first = b"x".to_vec();
    first.extend_from_slice(&snowman[..1]);
    buffer.push(data(0, &first)?);
    assert_eq!(buffer.take(10).output, "x");
    buffer.push(data(1, &snowman[1..])?);
    assert_eq!(buffer.take(10).output, "\u{2603}");
    Ok(())
}

#[test]
fn a_split_character_is_flushed_once_the_terminal_ended() -> TestResult {
    let mut buffer = Buffer::default();
    buffer.push(data(0, &"\u{2603}".as_bytes()[..1])?);
    buffer.push(exit(1, 0));
    let read = buffer.take(10);
    assert_eq!(read.output, "\u{fffd}");
    assert!(read.exited);
    Ok(())
}

#[test]
fn a_resync_frame_is_ignored_and_a_closed_stream_ends_without_a_code() {
    let mut buffer = Buffer::default();
    buffer.push(TerminalOutput::Resync { from_seq: 4 });
    assert!(!buffer.ready());
    buffer.close();
    assert!(buffer.ready());
    let read = buffer.take(10);
    assert!(read.exited);
    assert_eq!(read.exit_code, None);
}
