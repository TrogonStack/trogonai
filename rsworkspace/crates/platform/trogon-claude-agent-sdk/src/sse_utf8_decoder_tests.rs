use super::*;

#[test]
fn decodes_plain_text() {
    let mut decoder = SseUtf8Decoder::new();
    assert_eq!(decoder.decode("h\u{e9}llo".as_bytes(), true), "h\u{e9}llo");
}

#[test]
fn holds_a_sequence_cut_at_the_chunk_boundary() {
    let mut decoder = SseUtf8Decoder::new();
    let bytes = "\u{20ac}".as_bytes();
    assert_eq!(decoder.decode(&bytes[..2], true), "");
    assert_eq!(decoder.decode(&bytes[2..], true), "\u{20ac}");
}

#[test]
fn replaces_invalid_bytes() {
    let mut decoder = SseUtf8Decoder::new();
    assert_eq!(decoder.decode(&[b'a', 0xff, b'b'], true), "a\u{fffd}b");
}

#[test]
fn ends_with_a_replacement_for_an_incomplete_sequence() {
    let mut decoder = SseUtf8Decoder::new();
    assert_eq!(decoder.decode(&"\u{20ac}".as_bytes()[..2], false), "\u{fffd}");
}

#[test]
fn drops_one_leading_byte_order_mark_until_a_final_decode() {
    let mut decoder = SseUtf8Decoder::new();
    assert_eq!(decoder.decode(&[], true), "");
    assert_eq!(decoder.decode("\u{feff}a".as_bytes(), true), "a");
    assert_eq!(decoder.decode("\u{feff}b".as_bytes(), true), "\u{feff}b");
    assert_eq!(decoder.decode(&[], false), "");
    assert_eq!(decoder.decode("\u{feff}c".as_bytes(), false), "c");
}
