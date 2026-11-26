mod common;

use common::TestResult;
use trogon_sandbox_contract::OutputTruncation;
use trogon_sandbox_protocol::{
    CappedOutput, DEFAULT_MAX_OUTPUT_BYTES, cap_buffered, effective_max_output_bytes, recap,
};

#[test]
fn output_within_the_cap_is_kept_whole() {
    let mut out = CappedOutput::new(16);
    out.push(b"hello ");
    out.push(b"world");
    assert_eq!(out.truncation(), OutputTruncation::Complete);
    assert_eq!(out.into_bytes(), b"hello world");
}

#[test]
fn overflow_keeps_head_and_tail_and_drops_the_middle() -> TestResult {
    let mut out = CappedOutput::new(8);
    out.push(b"AAAA");
    out.push(b"middle-middle");
    out.push(b"ZZZZ");
    assert_eq!(out.truncation(), OutputTruncation::Truncated);
    let text = String::from_utf8(out.into_bytes())?;
    assert!(text.starts_with("AAAA\n[... 13 bytes truncated ...]\n"), "{text}");
    assert!(text.ends_with("ZZZZ"), "{text}");
    Ok(())
}

#[test]
fn a_single_oversized_chunk_is_split_across_head_and_tail() -> TestResult {
    let mut out = CappedOutput::new(6);
    out.push(b"0123456789");
    assert_eq!(out.truncation(), OutputTruncation::Truncated);
    let text = String::from_utf8(out.into_bytes())?;
    assert!(text.starts_with("012\n[... 4 bytes truncated ...]\n"), "{text}");
    assert!(text.ends_with("789"), "{text}");
    Ok(())
}

#[test]
fn retained_memory_stays_bounded_over_many_chunks() -> TestResult {
    let mut out = CappedOutput::new(1024);
    for _ in 0..10_000 {
        out.push(&[b'x'; 100]);
    }
    let text = String::from_utf8(out.into_bytes())?;
    assert!(text.contains(&format!("[... {} bytes truncated ...]", 1_000_000 - 1024)));
    assert!(text.len() < 1024 + 64);
    Ok(())
}

#[test]
fn skip_marks_a_caller_side_gap_between_head_and_tail() -> TestResult {
    let mut out = CappedOutput::new(100);
    out.push(b"head");
    out.skip(7);
    out.push(b"tail");
    assert_eq!(out.truncation(), OutputTruncation::Truncated);
    assert_eq!(
        String::from_utf8(out.into_bytes())?,
        "head\n[... 7 bytes truncated ...]\ntail"
    );
    Ok(())
}

#[test]
fn zero_cap_retains_nothing_but_reports_truncation() {
    let mut out = CappedOutput::new(0);
    out.push(b"abc");
    assert_eq!(out.truncation(), OutputTruncation::Truncated);
}

#[test]
fn buffered_output_is_capped_only_when_over() {
    assert_eq!(
        cap_buffered(b"abc".to_vec(), 3),
        (b"abc".to_vec(), OutputTruncation::Complete)
    );
    let (bytes, truncation) = cap_buffered(b"0123456789".to_vec(), 4);
    assert_eq!(truncation, OutputTruncation::Truncated);
    assert!(bytes.starts_with(b"01"));
    assert!(bytes.ends_with(b"89"));
}

#[test]
fn cuts_never_split_a_utf8_character() -> TestResult {
    let (bytes, truncation) = cap_buffered("aééééz".as_bytes().to_vec(), 4);
    assert_eq!(truncation, OutputTruncation::Truncated);
    assert_eq!(String::from_utf8(bytes)?, "a\n[... 8 bytes truncated ...]\nz");
    Ok(())
}

#[test]
fn recap_keeps_one_marker_with_the_running_total() -> TestResult {
    let (once, _) = cap_buffered(b"0123456789abcdefghij".to_vec(), 10);
    assert_eq!(
        String::from_utf8(once.clone())?,
        "01234\n[... 10 bytes truncated ...]\nfghij"
    );
    let (twice, truncation) = recap(once, 4);
    assert_eq!(truncation, OutputTruncation::Truncated);
    assert_eq!(String::from_utf8(twice)?, "01\n[... 16 bytes truncated ...]\nij");
    Ok(())
}

#[test]
fn recap_of_small_untruncated_output_is_a_no_op() -> TestResult {
    assert_eq!(recap(b"abc".to_vec(), 3), (b"abc".to_vec(), OutputTruncation::Complete));
    let (bytes, truncation) = recap(b"0123456789".to_vec(), 4);
    assert_eq!(truncation, OutputTruncation::Truncated);
    assert_eq!(String::from_utf8(bytes)?, "01\n[... 6 bytes truncated ...]\n89");
    Ok(())
}

#[test]
fn effective_cap_defaults_and_clamps_to_the_ceiling() {
    assert_eq!(effective_max_output_bytes(None, usize::MAX), DEFAULT_MAX_OUTPUT_BYTES);
    assert_eq!(effective_max_output_bytes(Some(10), 100), 10);
    assert_eq!(effective_max_output_bytes(Some(1000), 100), 100);
    assert_eq!(effective_max_output_bytes(None, 100), 100);
}

#[test]
fn a_recap_that_fits_keeps_the_prior_marker_and_reports_truncation() -> TestResult {
    let data = b"head\n[... 5 bytes truncated ...]\ntail".to_vec();
    let (bytes, truncation) = recap(data.clone(), 64);
    assert_eq!(truncation, OutputTruncation::Truncated);
    assert_eq!(bytes, data);
    Ok(())
}

#[test]
fn text_that_only_resembles_a_marker_is_not_mistaken_for_one() -> TestResult {
    let data = b"x\n[... nope\n[... 7 bytes truncated ...]\nend".to_vec();
    let (bytes, truncation) = recap(data.clone(), 64);
    assert_eq!(truncation, OutputTruncation::Truncated);
    assert_eq!(bytes, data);
    Ok(())
}

#[test]
fn a_cut_never_splits_a_multibyte_character() -> TestResult {
    let three = "\u{20ac}".repeat(10).into_bytes();
    let (bytes, truncation) = recap(three.clone(), 9);
    assert_eq!(truncation, OutputTruncation::Truncated);
    assert_eq!(
        String::from_utf8(bytes)?,
        "\u{20ac}\n[... 24 bytes truncated ...]\n\u{20ac}"
    );
    let (bytes, _) = recap(three, 11);
    assert_eq!(
        String::from_utf8(bytes)?,
        "\u{20ac}\n[... 21 bytes truncated ...]\n\u{20ac}\u{20ac}"
    );
    let four = "\u{1f600}".repeat(5).into_bytes();
    let (bytes, _) = recap(four, 10);
    assert_eq!(
        String::from_utf8(bytes)?,
        "\u{1f600}\n[... 12 bytes truncated ...]\n\u{1f600}"
    );
    Ok(())
}

#[test]
fn bytes_that_are_not_utf8_are_cut_where_they_fall() {
    let (bytes, truncation) = recap(vec![0xff; 20], 10);
    assert_eq!(truncation, OutputTruncation::Truncated);
    let mut expected = vec![0xff; 5];
    expected.extend_from_slice(b"\n[... 10 bytes truncated ...]\n");
    expected.extend_from_slice(&[0xff; 5]);
    assert_eq!(bytes, expected);
}
