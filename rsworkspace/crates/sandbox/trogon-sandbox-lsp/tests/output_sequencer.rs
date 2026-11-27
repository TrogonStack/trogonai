use trogon_sandbox_lsp::{Admission, LspError, OutputChunk, OutputKind, OutputSequencer};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn chunk(kind: OutputKind, seq: u64) -> OutputChunk {
    OutputChunk {
        kind,
        seq,
        data: vec![],
    }
}

#[test]
fn accepts_in_order_and_drops_duplicates() -> TestResult {
    let mut s = OutputSequencer::default();
    assert_eq!(s.admit(&chunk(OutputKind::Stdout, 0))?, Admission::Accept);
    assert_eq!(s.admit(&chunk(OutputKind::Stdout, 1))?, Admission::Accept);
    assert_eq!(s.admit(&chunk(OutputKind::Stdout, 1))?, Admission::Skip);
    assert_eq!(s.admit(&chunk(OutputKind::Stdout, 0))?, Admission::Skip);
    assert_eq!(s.admit(&chunk(OutputKind::Stdout, 2))?, Admission::Accept);
    Ok(())
}

#[test]
fn a_gap_names_the_missing_seq() -> TestResult {
    let mut s = OutputSequencer::default();
    s.admit(&chunk(OutputKind::Stdout, 0))?;
    let err = s.admit(&chunk(OutputKind::Stdout, 3)).err().ok_or("no error")?;
    assert!(matches!(err, LspError::Protocol(m) if m == "missing stdout seq 1 (received seq 3)"));
    Ok(())
}

#[test]
fn the_first_stdout_chunk_must_be_seq_zero() {
    let mut s = OutputSequencer::default();
    let err = s.admit(&chunk(OutputKind::Stdout, 2));
    assert!(matches!(err, Err(LspError::Protocol(m)) if m.contains("missing stdout seq 0")));
}

#[test]
fn streams_are_counted_independently() -> TestResult {
    let mut s = OutputSequencer::default();
    s.admit(&chunk(OutputKind::Stdout, 0))?;
    assert_eq!(s.admit(&chunk(OutputKind::Stderr, 0))?, Admission::Accept);
    assert_eq!(s.admit(&chunk(OutputKind::Stderr, 0))?, Admission::Skip);
    assert_eq!(s.admit(&chunk(OutputKind::Stderr, 5))?, Admission::Accept);
    assert_eq!(s.admit(&chunk(OutputKind::Stdout, 1))?, Admission::Accept);
    Ok(())
}
