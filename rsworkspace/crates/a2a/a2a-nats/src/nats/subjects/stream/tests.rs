use super::*;

#[test]
fn display_renders_stream_suffix() {
    assert_eq!(A2aStream::Events.to_string(), "EVENTS");
    assert_eq!(A2aStream::PushDlq.to_string(), "PUSH_DLQ");
}
