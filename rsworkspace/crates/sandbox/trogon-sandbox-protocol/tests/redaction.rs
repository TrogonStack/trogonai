mod common;

use common::TestResult;
use trogon_sandbox_contract::{RedactValue, RedactValues};
use trogon_sandbox_protocol::{MIN_REDACTABLE_LEN, REDACTION_MARKER, RedactionSet};

const SECRET: &str = "s3cr3t-token-value";

fn set(values: &[&str]) -> RedactionSet {
    RedactionSet::new(values)
}

fn stream_all(set: &RedactionSet, chunks: &[&[u8]]) -> Vec<u8> {
    let mut stream = set.stream();
    let mut out = Vec::new();
    for chunk in chunks {
        out.extend(stream.push(chunk));
    }
    out.extend(stream.finish());
    out
}

#[test]
fn the_marker_is_fixed() {
    assert_eq!(REDACTION_MARKER, "[REDACTED]");
}

#[test]
fn replaces_a_registered_value() {
    let out = set(&[SECRET]).redact(format!("key={SECRET}\n").as_bytes());
    assert_eq!(out, b"key=[REDACTED]\n");
}

#[test]
fn replaces_every_occurrence() {
    let input = format!("{SECRET} and {SECRET}");
    assert_eq!(set(&[SECRET]).redact(input.as_bytes()), b"[REDACTED] and [REDACTED]");
}

#[test]
fn output_without_a_value_is_unchanged() {
    assert_eq!(set(&[SECRET]).redact(b"nothing here"), b"nothing here");
}

#[test]
fn values_below_the_minimum_length_are_ignored() {
    let short = "a".repeat(MIN_REDACTABLE_LEN - 1);
    let redaction = set(&[&short]);
    assert!(redaction.is_empty());
    assert_eq!(redaction.redact(b"aaaaa aaaaa"), b"aaaaa aaaaa");
}

#[test]
fn a_value_at_the_minimum_length_is_redacted() {
    let exact = "b".repeat(MIN_REDACTABLE_LEN);
    assert_eq!(set(&[&exact]).redact(exact.as_bytes()), b"[REDACTED]");
}

#[test]
fn empty_set_is_a_no_op() {
    let redaction = RedactionSet::default();
    assert!(redaction.is_empty());
    assert_eq!(redaction.redact(b"abc"), b"abc");
    assert_eq!(stream_all(&redaction, &[b"ab", b"c"]), b"abc");
}

#[test]
fn longest_value_wins_when_one_contains_another() {
    let redaction = set(&["abcdef", "abcdefghij"]);
    assert_eq!(redaction.redact(b"x abcdefghij y"), b"x [REDACTED] y");
}

#[test]
fn duplicates_do_not_change_the_result() {
    assert_eq!(set(&[SECRET, SECRET]).redact(SECRET.as_bytes()), b"[REDACTED]");
}

#[test]
fn registration_order_does_not_change_the_result() {
    let a = set(&["abcdef", "abcdefghij", "zzzzzz"]);
    let b = set(&["zzzzzz", "abcdefghij", "abcdef"]);
    let input = b"abcdefghij abcdef zzzzzz abcde";
    assert_eq!(a.redact(input), b.redact(input));
}

#[test]
fn adjacent_values_are_each_replaced() {
    let redaction = set(&["aaaaaa", "bbbbbb"]);
    assert_eq!(redaction.redact(b"aaaaaabbbbbb"), b"[REDACTED][REDACTED]");
}

#[test]
fn overlapping_occurrences_are_replaced_leftmost_first() {
    assert_eq!(set(&["abcabc"]).redact(b"abcabcabc"), b"[REDACTED]abc");
}

#[test]
fn non_ascii_values_are_redacted_by_bytes() {
    let value = "pässwörd-ключ";
    assert_eq!(set(&[value]).redact_str(&format!("x {value} y")), "x [REDACTED] y");
}

#[test]
fn redact_str_scrubs_error_text() {
    let message = format!("exec failed: env TOKEN={SECRET} rejected");
    assert_eq!(
        set(&[SECRET]).redact_str(&message),
        "exec failed: env TOKEN=[REDACTED] rejected"
    );
}

#[test]
fn stream_catches_a_value_split_across_two_chunks() {
    let redaction = set(&[SECRET]);
    let (a, b) = SECRET.as_bytes().split_at(7);
    let mut input_a = b"pre ".to_vec();
    input_a.extend_from_slice(a);
    let mut input_b = b.to_vec();
    input_b.extend_from_slice(b" post");
    assert_eq!(stream_all(&redaction, &[&input_a, &input_b]), b"pre [REDACTED] post");
}

#[test]
fn stream_catches_a_value_split_one_byte_at_a_time() {
    let redaction = set(&[SECRET]);
    let input = format!("a {SECRET} b");
    let chunks: Vec<&[u8]> = input.as_bytes().chunks(1).collect();
    assert_eq!(stream_all(&redaction, &chunks), b"a [REDACTED] b");
}

#[test]
fn stream_holds_back_at_most_max_len_minus_one_bytes() {
    let redaction = set(&[SECRET]);
    let mut stream = redaction.stream();
    let mut out = stream.push(&[b'x'; 100]);
    assert!(stream.held_back() < SECRET.len());
    out.extend(stream.finish());
    assert_eq!(out, vec![b'x'; 100]);
}

#[test]
fn stream_emits_safe_bytes_without_waiting_for_finish() {
    let redaction = set(&[SECRET]);
    let mut stream = redaction.stream();
    let out = stream.push(&[b'x'; 100]);
    assert_eq!(out.len(), 100 - stream.held_back());
    assert!(!out.is_empty());
}

#[test]
fn stream_flushes_a_partial_prefix_at_the_end() {
    let redaction = set(&[SECRET]);
    let partial = &SECRET.as_bytes()[..5];
    assert_eq!(stream_all(&redaction, &[partial]), partial);
}

#[test]
fn stream_reassembles_a_false_start() {
    let redaction = set(&[SECRET]);
    let input = format!("s3cr3t-token-valuX {SECRET}");
    let (a, b) = input.as_bytes().split_at(10);
    assert_eq!(
        stream_all(&redaction, &[a, b]),
        b"s3cr3t-token-valuX [REDACTED]".to_vec()
    );
}

#[test]
fn every_two_way_split_matches_whole_buffer_redaction() {
    let redaction = set(&["abcabc", "abcdefghij", SECRET]);
    let input = format!("zzabcabcabcdefghijabcab{SECRET}abcabcab").into_bytes();
    let whole = redaction.redact(&input);
    for cut in 0..=input.len() {
        let (a, b) = input.split_at(cut);
        assert_eq!(stream_all(&redaction, &[a, b]), whole, "cut at {cut}");
    }
}

#[test]
fn every_chunk_size_matches_whole_buffer_redaction() {
    let redaction = set(&["abcabc", "abcdefghij", SECRET]);
    let input = format!("abcabcabc {SECRET}{SECRET} abcdefghijabcdefghi abcab").into_bytes();
    let whole = redaction.redact(&input);
    for size in 1..=input.len() {
        let chunks: Vec<&[u8]> = input.chunks(size).collect();
        assert_eq!(stream_all(&redaction, &chunks), whole, "chunk size {size}");
    }
}

#[test]
fn empty_chunks_are_harmless() {
    let redaction = set(&[SECRET]);
    let (a, b) = SECRET.as_bytes().split_at(4);
    assert_eq!(stream_all(&redaction, &[a, b"", b"", b]), b"[REDACTED]");
}

#[test]
fn encoded_values_are_not_caught() {
    let hex: String = SECRET.bytes().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(set(&[SECRET]).redact(hex.as_bytes()), hex.as_bytes());
}

#[test]
fn debug_never_shows_the_value() -> TestResult {
    let value = RedactValue::new(SECRET);
    assert!(!format!("{value:?}").contains(SECRET));
    let redaction = RedactionSet::from_request_values(&RedactValues::new(vec![value])?);
    assert!(!format!("{redaction:?}").contains(SECRET));
    assert_eq!(redaction.redact(SECRET.as_bytes()), b"[REDACTED]");
    Ok(())
}
