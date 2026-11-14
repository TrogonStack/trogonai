use super::FrameReader;
use crate::{Error, error::JsonDecodeSourceError};

async fn frames(input: &[u8], limit: Option<usize>) -> Result<Vec<Vec<u8>>, Error> {
    let mut reader = FrameReader::new(input, limit);
    let mut out = Vec::new();
    while let Some(frame) = reader.next().await? {
        out.push(frame);
    }
    Ok(out)
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
