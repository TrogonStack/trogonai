use super::*;

fn frame(event: Option<&str>, id: Option<&str>, data: Option<&str>) -> Option<SseFrame> {
    Some(SseFrame {
        event: event.map(str::to_owned),
        id: id.map(str::to_owned),
        data: data.map(str::to_owned),
    })
}

#[test]
fn reads_fields_and_strips_one_leading_space() {
    assert_eq!(
        SseFrame::parse("event: a\nid:  b\ndata:c"),
        frame(Some("a"), Some(" b"), Some("c"))
    );
}

#[test]
fn joins_data_lines_with_a_newline() {
    assert_eq!(
        SseFrame::parse("data: a\r\ndata: b\rdata:\ndata: c"),
        frame(None, None, Some("a\nb\n\nc"))
    );
}

#[test]
fn an_empty_first_data_line_is_not_joined() {
    assert_eq!(SseFrame::parse("data:\ndata: b"), frame(None, None, Some("b")));
}

#[test]
fn a_comment_alone_is_a_frame() {
    assert_eq!(SseFrame::parse(": keepalive"), frame(None, None, None));
}

#[test]
fn blocks_without_data_or_comment_are_no_frame() {
    assert_eq!(SseFrame::parse("  \n\t"), None);
    assert_eq!(SseFrame::parse("event: a\nid: 1"), None);
    assert_eq!(SseFrame::parse("data:\nnocolon\nother: x"), None);
}
