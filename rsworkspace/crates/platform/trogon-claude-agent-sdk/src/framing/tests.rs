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
