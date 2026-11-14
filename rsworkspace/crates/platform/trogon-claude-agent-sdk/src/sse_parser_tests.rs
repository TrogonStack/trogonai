use super::*;

fn data(frames: &[SseFrame]) -> Vec<&str> {
    frames.iter().filter_map(|frame| frame.data.as_deref()).collect()
}

#[test]
fn splits_frames_at_blank_lines_of_any_ending() {
    let mut parser = SseParser::new();
    let frames = parser.push(b"data: a\n\ndata: b\r\n\r\ndata: c\r\rdata: d");
    assert_eq!(data(&frames), ["a", "b", "c"]);
    assert_eq!(parser.buffered(), "data: d");
    assert_eq!(parser.pending_chars(), 7);
    assert_eq!(parser.frames_dispatched(), 3);
}

#[test]
fn a_carriage_return_at_the_chunk_boundary_waits_for_its_partner() {
    let mut parser = SseParser::new();
    assert!(parser.push(b"data: a\r\n\r").is_empty());
    assert_eq!(data(&parser.push(b"\ndata: b\n\n")), ["a", "b"]);
}

#[test]
fn accumulates_across_chunks_and_ignores_empty_ones() {
    let mut parser = SseParser::new();
    assert!(parser.push_str("").is_empty());
    assert!(parser.push(b"da").is_empty());
    assert!(parser.push(b"ta: x\n").is_empty());
    assert_eq!(data(&parser.push(b"\n")), ["x"]);
}

#[test]
fn counts_delimiters_without_frames() {
    let mut parser = SseParser::new();
    assert!(parser.push(b"event: a\n\n").is_empty());
    assert_eq!(parser.frames_dispatched(), 1);
}

#[test]
fn flush_parses_the_rest_and_starts_over() {
    let mut parser = SseParser::default();
    parser.push(b"data: tail\xe2\x82");
    let frames = parser.flush();
    assert_eq!(data(&frames), ["tail\u{fffd}"]);
    assert_eq!(parser.buffered(), "");
    assert!(parser.flush().is_empty());
}
