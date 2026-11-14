use super::{FrameReader, typescript_lines};
use crate::python_urlopen_text::python_decode;
use crate::{ArgvStyle, Error, error::JsonDecodeSourceError};
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};

async fn frames(input: &[u8], limit: Option<usize>) -> Result<Vec<Vec<u8>>, Error> {
    let style = if limit.is_some() {
        ArgvStyle::Python
    } else {
        ArgvStyle::TypeScript
    };
    read_all(FrameReader::new(input, limit, style)).await
}

async fn read_all<R: AsyncRead + Unpin>(mut reader: FrameReader<R>) -> Result<Vec<Vec<u8>>, Error> {
    let mut out = Vec::new();
    while let Some(frame) = reader.next().await? {
        out.push(frame);
    }
    Ok(out)
}

/// A pipe that yields these chunks, one per read, as the CLI's writes arrive.
struct Chunks(VecDeque<Vec<u8>>);

impl AsyncRead for Chunks {
    fn poll_read(mut self: Pin<&mut Self>, _: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        if let Some(chunk) = self.0.pop_front() {
            buf.put_slice(&chunk);
        }
        Poll::Ready(Ok(()))
    }
}

fn python_chunks(chunks: &[&[u8]], limit: usize) -> FrameReader<Chunks> {
    let chunks = Chunks(chunks.iter().map(|chunk| chunk.to_vec()).collect());
    FrameReader::new(chunks, Some(limit), ArgvStyle::Python)
}

#[test]
fn python_decodes_each_chunk_strictly_and_holds_back_a_cut_character() {
    assert_eq!(python_decode("aé".as_bytes()), Ok(0));
    assert_eq!(python_decode(b"a\xe2\x82"), Ok(2));
    assert_eq!(
        python_decode(b"ab\xff"),
        Err("'utf-8' codec can't decode byte 0xff in position 2: invalid start byte".to_owned())
    );
    assert_eq!(
        python_decode(b"a\xc0\x80"),
        Err("'utf-8' codec can't decode byte 0xc0 in position 1: invalid start byte".to_owned())
    );
    assert_eq!(
        python_decode(b"\xed\xa0\x80"),
        Err("'utf-8' codec can't decode byte 0xed in position 0: invalid continuation byte".to_owned())
    );
    assert_eq!(
        python_decode(b"x\xe2\x82A"),
        Err("'utf-8' codec can't decode bytes in position 1-2: invalid continuation byte".to_owned())
    );
}

#[tokio::test]
async fn python_raises_for_a_chunk_that_is_not_utf_8_before_the_lines_in_it() -> Result<(), std::io::Error> {
    let mut reader = python_chunks(&[b"one\n", b"two\nthree\xff\n"], 100);
    assert_eq!(reader.next().await.ok().flatten(), Some(b"one".to_vec()));
    let Err(Error::UnicodeDecode(text)) = reader.next().await else {
        return Err(std::io::Error::other("expected UnicodeDecodeError"));
    };
    assert_eq!(
        text,
        "'utf-8' codec can't decode byte 0xff in position 9: invalid start byte"
    );
    Ok(())
}

#[tokio::test]
async fn python_counts_a_held_character_from_the_chunk_that_completes_it() -> Result<(), Error> {
    // The guard sees four characters, then four again once the cut "é" completes.
    let reader = python_chunks(&[b"abc\xc3", b"\xa9\n", b"abcd\xc3"], 4);
    assert_eq!(read_all(reader).await?, ["abcé".as_bytes().to_vec(), b"abcd".to_vec()]);
    let reader = python_chunks(&[b"abcd\xc3", b"\xa9"], 4);
    assert!(matches!(read_all(reader).await, Err(Error::JsonDecode(_))));
    let reader = python_chunks(&[b"\xc3"], 4);
    assert_eq!(read_all(reader).await?, Vec::<Vec<u8>>::new());
    Ok(())
}

#[tokio::test]
async fn python_raises_with_the_position_in_the_held_bytes_and_the_new_chunk() -> Result<(), std::io::Error> {
    let reader = python_chunks(&[b"a\xe2", b"\x82x\n"], 100);
    let Err(Error::UnicodeDecode(text)) = read_all(reader).await else {
        return Err(std::io::Error::other("expected UnicodeDecodeError"));
    };
    assert_eq!(
        text,
        "'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte"
    );
    Ok(())
}

#[tokio::test]
async fn typescript_ends_a_line_at_a_lone_carriage_return_and_the_unicode_separators_too() -> Result<(), Error> {
    assert_eq!(typescript_lines(b"ab".to_vec()), [b"ab".to_vec()]);
    assert_eq!(
        typescript_lines(b"a\rb\r".to_vec()),
        [b"a".to_vec(), b"b".to_vec(), Vec::new()]
    );
    assert_eq!(
        typescript_lines(b"\xe2\x80\xa8a\xe2\x80\xa9\xe2\x80\xa7\xc3\xa9\xe2\xe2\x80\xa8".to_vec()),
        [
            Vec::new(),
            b"a".to_vec(),
            b"\xe2\x80\xa7\xc3\xa9\xe2".to_vec(),
            Vec::new()
        ]
    );
    assert_eq!(typescript_lines(b"\xe2\x80".to_vec()), [b"\xe2\x80".to_vec()]);
    assert_eq!(
        frames(b"a\rb\r\nc\r\r\n\xff\n", None).await?,
        [
            b"a".to_vec(),
            b"b".to_vec(),
            b"c".to_vec(),
            Vec::new(),
            b"\xff".to_vec()
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_line_of_exactly_the_limit_is_accepted_without_its_newline() -> Result<(), Error> {
    assert_eq!(
        frames(b"abcd\nefgh", Some(4)).await?,
        [b"abcd".to_vec(), b"efgh".to_vec()]
    );
    Ok(())
}

#[tokio::test]
async fn a_longer_line_is_python_s_buffer_guard() -> Result<(), std::io::Error> {
    let Err(Error::JsonDecode(error)) = frames(b"abcde\n", Some(4)).await else {
        return Err(std::io::Error::other("expected the buffer guard"));
    };
    assert_eq!(error.line(), "JSON message exceeded maximum buffer size of 4 bytes");
    assert_eq!(
        error.to_string(),
        "Failed to decode JSON: JSON message exceeded maximum buffer size of 4 bytes..."
    );
    assert!(matches!(
        error.original_error(),
        JsonDecodeSourceError::BufferExceeded { length: 5, limit: 4 }
    ));
    assert_eq!(error.original_error().to_string(), "Buffer size 5 exceeds limit 4");
    Ok(())
}

#[tokio::test]
async fn the_limit_counts_characters_not_bytes() -> Result<(), Error> {
    // Four characters, eight bytes.
    assert_eq!(
        frames("éééé\n".as_bytes(), Some(4)).await?,
        ["éééé".as_bytes().to_vec()]
    );
    Ok(())
}

#[tokio::test]
async fn without_a_limit_every_line_is_kept() -> Result<(), Error> {
    let long = vec![b'x'; 100_000];
    let mut input = long.clone();
    input.push(b'\n');
    assert_eq!(frames(&input, None).await?, [long]);
    Ok(())
}

#[test]
fn an_invalid_byte_becomes_a_replacement_character_without_holding_later_bytes() {
    let mut pending = Vec::new();
    assert_eq!(super::decode_chunk(&mut pending, b"a\xffb"), "a\u{fffd}b");
    assert!(pending.is_empty());
}

#[test]
fn an_incomplete_character_waits_for_the_next_chunk() {
    let mut pending = Vec::new();
    let bytes = "é".as_bytes();
    assert_eq!(super::decode_chunk(&mut pending, &bytes[..1]), "");
    assert_eq!(super::decode_chunk(&mut pending, &bytes[1..]), "é");
    assert!(pending.is_empty());
}

fn tail_text(tail: &super::SharedTail) -> String {
    String::from_utf8_lossy(tail.snapshot().as_bytes()).into_owned()
}

#[test]
fn the_tail_keeps_the_last_units_without_surrounding_whitespace() {
    let tail = super::SharedTail::new(8, false);
    tail.append(b"  header line\nlast words \n");
    assert_eq!(tail_text(&tail), "words");
}

#[test]
fn the_tail_trims_the_javascript_whitespace_set_only() {
    let tail = super::SharedTail::new(64, false);
    tail.append("\u{feff}\u{a0} body \u{85}\u{3000}".as_bytes());
    assert_eq!(tail_text(&tail), "body \u{85}");
}

#[test]
fn a_surrogate_pair_cut_in_half_is_dropped() {
    let tail = super::SharedTail::new(3, false);
    tail.append("a\u{1f600}bc".as_bytes());
    assert_eq!(tail_text(&tail), "bc");
}

#[test]
fn the_tail_is_cut_once_it_holds_more_than_twice_the_units() {
    let tail = super::SharedTail::new(4, false);
    tail.append(b"12345678");
    assert_eq!(tail_text(&tail), "5678");
    tail.append(b"9");
    assert_eq!(tail_text(&tail), "6789");
}

#[test]
fn a_secret_is_removed_from_the_tail_only_when_redacting() {
    let secret = format!("sk-{}", "a1".repeat(12));
    let redacting = super::SharedTail::new(2048, true);
    redacting.append(format!("token {secret} end").as_bytes());
    assert_eq!(tail_text(&redacting), "token <token> end");
    let plain = super::SharedTail::new(2048, false);
    plain.append(format!("token {secret} end").as_bytes());
    assert_eq!(tail_text(&plain), format!("token {secret} end"));
}

#[test]
fn a_secret_is_removed_before_the_tail_is_cut() {
    let secret = format!("sk-{}", "a1".repeat(12));
    let tail = super::SharedTail::new(20, true);
    tail.append(format!("x {secret} yyyyyyyyyyyyyyyyyyyyyyyyy").as_bytes());
    assert_eq!(tail_text(&tail), "yyyyyyyyyyyyyyyyyyyy");
}

#[test]
fn a_stream_that_ends_inside_a_character_leaves_one_replacement_character() {
    let tail = super::SharedTail::new(64, false);
    tail.append(&"é".as_bytes()[..1]);
    assert_eq!(tail_text(&tail), "");
    tail.finish();
    assert_eq!(tail_text(&tail), "\u{fffd}");
    tail.finish();
    assert_eq!(tail_text(&tail), "\u{fffd}");
}

#[tokio::test]
async fn draining_stderr_fills_the_tail_and_flushes_a_cut_character() -> Result<(), std::io::Error> {
    let tail = super::SharedTail::new(64, false);
    let mut bytes = b"oops ".to_vec();
    bytes.push(0xC3);
    super::drain_stderr_with(&bytes[..], tail.clone(), 100, None).await?;
    assert_eq!(tail_text(&tail), "oops \u{fffd}");
    Ok(())
}
