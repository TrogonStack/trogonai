use super::*;

#[test]
fn buffered_output_keeps_both_streams() {
    let mut collector = OutputCollector::new(OutputRetention::Buffer, None);
    collector.push(OutputStream::Stdout, b"out");
    collector.push(OutputStream::Stderr, b"err");
    let output = collector.finish(3, None);
    assert_eq!(
        (output.exit_code, output.stdout, output.stderr),
        (3, b"out".to_vec(), b"err".to_vec())
    );
    assert!(!output.timed_out.is_timed_out());
    assert!(!output.truncated.is_truncated());
}

#[test]
fn stream_only_keeps_no_stdout_and_a_bounded_stderr_tail() {
    let mut collector = OutputCollector::new(OutputRetention::StreamOnly, None);
    collector.push(OutputStream::Stdout, b"dropped");
    let limit = OutputRetention::STREAM_ONLY_STDERR_TAIL_BYTES;
    collector.push(OutputStream::Stderr, &vec![b'a'; limit]);
    collector.push(OutputStream::Stderr, b"tail");
    let output = collector.finish(0, None);
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr.len(), limit);
    assert!(output.stderr.ends_with(b"tail"));
}

#[test]
fn a_cap_marks_the_output_truncated() {
    let mut collector = OutputCollector::new(OutputRetention::Buffer, Some(8));
    collector.push(OutputStream::Stdout, &[b'x'; 64]);
    assert!(collector.finish(0, None).truncated.is_truncated());
}

#[test]
fn a_timeout_overrides_the_exit_code_and_says_so_on_stderr() {
    let mut collector = OutputCollector::new(OutputRetention::Buffer, None);
    collector.push(OutputStream::Stderr, b"partial ");
    let output = collector.finish(137, Some(2));
    assert_eq!(output.exit_code, CommandOutput::TIMED_OUT_EXIT_CODE);
    assert!(output.timed_out.is_timed_out());
    assert_eq!(output.stderr, b"partial command timed out after 2s");
}
