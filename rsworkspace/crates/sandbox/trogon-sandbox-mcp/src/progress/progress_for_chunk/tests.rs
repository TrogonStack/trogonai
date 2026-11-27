use rmcp::model::{NumberOrString, ProgressToken};
use trogon_sandbox_contract::{LogChunk, OutputStream};

use super::progress_for_chunk;

fn token() -> ProgressToken {
    ProgressToken(NumberOrString::String("tok".into()))
}

fn chunk(log_seq: u64, stream: OutputStream, data: &[u8]) -> LogChunk {
    LogChunk {
        log_seq,
        stream,
        seq: log_seq,
        data: data.to_vec(),
    }
}

#[test]
fn a_chunk_becomes_a_progress_notification_with_its_text() {
    let out = progress_for_chunk(&token(), 3, &chunk(9, OutputStream::Stdout, b"hello"));
    assert_eq!(out.progress_token, token());
    assert_eq!(out.progress, 3.0);
    assert_eq!(out.total, None);
    assert_eq!(out.message.as_deref(), Some("hello"));

    let err = progress_for_chunk(&token(), 4, &chunk(10, OutputStream::Stderr, b"boom"));
    assert_eq!(err.message.as_deref(), Some("[stderr] boom"));
}

#[test]
fn non_utf8_output_is_replaced_not_dropped() {
    let out = progress_for_chunk(&token(), 1, &chunk(1, OutputStream::Stdout, &[b'a', 0xff, b'b']));
    assert_eq!(out.message.as_deref(), Some("a\u{fffd}b"));
}
