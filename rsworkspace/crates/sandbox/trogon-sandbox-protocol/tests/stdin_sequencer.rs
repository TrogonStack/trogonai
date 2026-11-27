use trogon_sandbox_contract::{StdinChunk, StdinEof, StdinSeq};
use trogon_sandbox_protocol::{StdinAdmit, StdinSequencer, StdinWrite};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn chunk(bytes: &[u8]) -> Result<StdinChunk, Box<dyn std::error::Error>> {
    Ok(StdinChunk::new(bytes.to_vec())?)
}

fn write(bytes: &[u8], eof: StdinEof) -> Result<StdinWrite, Box<dyn std::error::Error>> {
    Ok(StdinWrite {
        data: chunk(bytes)?,
        eof,
    })
}

fn seq(value: u64) -> StdinSeq {
    StdinSeq::new(value)
}

fn admit(
    sequencer: &mut StdinSequencer,
    value: u64,
    bytes: &[u8],
    eof: StdinEof,
) -> Result<StdinAdmit, Box<dyn std::error::Error>> {
    Ok(sequencer.admit(seq(value), chunk(bytes)?, eof))
}

fn apply(
    sequencer: &mut StdinSequencer,
    value: u64,
    bytes: &[u8],
    eof: StdinEof,
) -> Result<Vec<StdinWrite>, Box<dyn std::error::Error>> {
    match admit(sequencer, value, bytes, eof)? {
        StdinAdmit::Apply(writes) => Ok(writes),
        other => Err(format!("expected Apply, got {other:?}").into()),
    }
}

#[test]
fn in_order_writes_apply_one_by_one() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    assert_eq!(
        apply(&mut sequencer, 0, b"a", StdinEof::Keep)?,
        vec![write(b"a", StdinEof::Keep)?]
    );
    sequencer.commit(1);
    assert_eq!(sequencer.next_seq(), seq(1));
    assert_eq!(
        apply(&mut sequencer, 1, b"b", StdinEof::Keep)?,
        vec![write(b"b", StdinEof::Keep)?]
    );
    sequencer.commit(1);
    assert_eq!(sequencer.next_seq(), seq(2));
    assert!(!sequencer.is_closed());
    Ok(())
}

#[test]
fn admit_does_not_advance_until_commit() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    apply(&mut sequencer, 0, b"a", StdinEof::Keep)?;
    assert_eq!(sequencer.next_seq(), seq(0));
    let retry = apply(&mut sequencer, 0, b"a", StdinEof::Keep)?;
    assert_eq!(retry, vec![write(b"a", StdinEof::Keep)?]);
    Ok(())
}

#[test]
fn committed_seq_is_duplicate() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    apply(&mut sequencer, 0, b"a", StdinEof::Keep)?;
    sequencer.commit(1);
    assert_eq!(admit(&mut sequencer, 0, b"a", StdinEof::Keep)?, StdinAdmit::Duplicate);
    Ok(())
}

#[test]
fn early_write_is_buffered_then_drained_in_order() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    assert_eq!(admit(&mut sequencer, 2, b"c", StdinEof::Keep)?, StdinAdmit::Buffered);
    assert_eq!(admit(&mut sequencer, 1, b"b", StdinEof::Keep)?, StdinAdmit::Buffered);
    let writes = apply(&mut sequencer, 0, b"a", StdinEof::Keep)?;
    assert_eq!(
        writes,
        vec![
            write(b"a", StdinEof::Keep)?,
            write(b"b", StdinEof::Keep)?,
            write(b"c", StdinEof::Keep)?
        ]
    );
    sequencer.commit(3);
    assert_eq!(sequencer.next_seq(), seq(3));
    assert_eq!(admit(&mut sequencer, 2, b"c", StdinEof::Keep)?, StdinAdmit::Duplicate);
    Ok(())
}

#[test]
fn rebuffering_the_same_seq_is_buffered_and_keeps_first_bytes() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    assert_eq!(admit(&mut sequencer, 1, b"b", StdinEof::Keep)?, StdinAdmit::Buffered);
    assert_eq!(admit(&mut sequencer, 1, b"x", StdinEof::Keep)?, StdinAdmit::Buffered);
    let writes = apply(&mut sequencer, 0, b"a", StdinEof::Keep)?;
    assert_eq!(writes, vec![write(b"a", StdinEof::Keep)?, write(b"b", StdinEof::Keep)?]);
    Ok(())
}

#[test]
fn partial_commit_keeps_the_rest_buffered() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    admit(&mut sequencer, 1, b"b", StdinEof::Keep)?;
    apply(&mut sequencer, 0, b"a", StdinEof::Keep)?;
    sequencer.commit(1);
    assert_eq!(sequencer.next_seq(), seq(1));
    let writes = apply(&mut sequencer, 1, b"b", StdinEof::Keep)?;
    assert_eq!(writes, vec![write(b"b", StdinEof::Keep)?]);
    Ok(())
}

#[test]
fn write_beyond_window_is_out_of_window() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    assert_eq!(
        admit(
            &mut sequencer,
            StdinSequencer::REORDER_WINDOW_CHUNKS,
            b"x",
            StdinEof::Keep
        )?,
        StdinAdmit::Buffered
    );
    assert_eq!(
        admit(
            &mut sequencer,
            StdinSequencer::REORDER_WINDOW_CHUNKS + 1,
            b"x",
            StdinEof::Keep
        )?,
        StdinAdmit::OutOfWindow
    );
    Ok(())
}

#[test]
fn window_is_relative_to_next_seq() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    apply(&mut sequencer, 0, b"a", StdinEof::Keep)?;
    sequencer.commit(1);
    assert_eq!(
        admit(
            &mut sequencer,
            1 + StdinSequencer::REORDER_WINDOW_CHUNKS,
            b"x",
            StdinEof::Keep
        )?,
        StdinAdmit::Buffered
    );
    assert_eq!(
        admit(
            &mut sequencer,
            2 + StdinSequencer::REORDER_WINDOW_CHUNKS,
            b"x",
            StdinEof::Keep
        )?,
        StdinAdmit::OutOfWindow
    );
    Ok(())
}

fn fill_window(sequencer: &mut StdinSequencer) -> Result<u64, Box<dyn std::error::Error>> {
    let full = vec![0u8; StdinChunk::MAX_LEN];
    let per_window = (StdinSequencer::REORDER_WINDOW_BYTES / StdinChunk::MAX_LEN) as u64;
    for n in 0..per_window {
        assert_eq!(
            sequencer.admit(seq(n + 1), StdinChunk::new(full.clone())?, StdinEof::Keep),
            StdinAdmit::Buffered
        );
    }
    Ok(per_window)
}

#[test]
fn buffered_bytes_are_capped() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    let per_window = fill_window(&mut sequencer)?;
    assert_eq!(
        admit(&mut sequencer, per_window + 1, b"x", StdinEof::Keep)?,
        StdinAdmit::OutOfWindow
    );
    Ok(())
}

#[test]
fn commit_releases_buffered_bytes() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    let per_window = fill_window(&mut sequencer)?;
    let writes = apply(&mut sequencer, 0, b"a", StdinEof::Keep)?;
    sequencer.commit(writes.len());
    assert_eq!(
        sequencer.admit(
            seq(per_window),
            StdinChunk::new(vec![0u8; StdinChunk::MAX_LEN])?,
            StdinEof::Keep
        ),
        StdinAdmit::Duplicate
    );
    assert_eq!(
        admit(&mut sequencer, per_window + 1, b"y", StdinEof::Keep)?,
        StdinAdmit::Apply(vec![write(b"y", StdinEof::Keep)?])
    );
    Ok(())
}

#[test]
fn eof_closes_stdin_after_commit() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    let writes = apply(&mut sequencer, 0, b"bye", StdinEof::Close)?;
    assert_eq!(writes, vec![write(b"bye", StdinEof::Close)?]);
    assert!(!sequencer.is_closed());
    sequencer.commit(1);
    assert!(sequencer.is_closed());
    assert_eq!(admit(&mut sequencer, 1, b"x", StdinEof::Keep)?, StdinAdmit::Closed);
    assert_eq!(
        admit(&mut sequencer, 0, b"bye", StdinEof::Close)?,
        StdinAdmit::Duplicate
    );
    Ok(())
}

#[test]
fn eof_without_data_is_applied() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    let writes = apply(&mut sequencer, 0, b"", StdinEof::Close)?;
    assert_eq!(writes, vec![write(b"", StdinEof::Close)?]);
    Ok(())
}

#[test]
fn drain_stops_at_eof_and_discards_later_buffered_writes() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    admit(&mut sequencer, 1, b"b", StdinEof::Close)?;
    admit(&mut sequencer, 2, b"c", StdinEof::Keep)?;
    let writes = apply(&mut sequencer, 0, b"a", StdinEof::Keep)?;
    assert_eq!(
        writes,
        vec![write(b"a", StdinEof::Keep)?, write(b"b", StdinEof::Close)?]
    );
    sequencer.commit(2);
    assert!(sequencer.is_closed());
    assert_eq!(sequencer.next_seq(), seq(2));
    assert_eq!(admit(&mut sequencer, 2, b"c", StdinEof::Keep)?, StdinAdmit::Closed);
    Ok(())
}

#[test]
fn failed_apply_can_be_retried_with_the_same_seq() -> TestResult {
    let mut sequencer = StdinSequencer::new();
    apply(&mut sequencer, 0, b"a", StdinEof::Keep)?;
    sequencer.commit(0);
    assert_eq!(sequencer.next_seq(), seq(0));
    assert_eq!(
        admit(&mut sequencer, 0, b"a", StdinEof::Keep)?,
        StdinAdmit::Apply(vec![write(b"a", StdinEof::Keep)?])
    );
    Ok(())
}

#[test]
fn commit_without_a_pending_run_changes_nothing() {
    let mut sequencer = StdinSequencer::new();
    sequencer.commit(3);
    assert_eq!(sequencer.next_seq(), seq(0));
    assert!(!sequencer.is_closed());
}

#[test]
fn admit_is_deterministic_for_identical_input() -> TestResult {
    let mut a = StdinSequencer::new();
    let mut b = StdinSequencer::new();
    for sequencer in [&mut a, &mut b] {
        admit(sequencer, 2, b"c", StdinEof::Keep)?;
        admit(sequencer, 1, b"b", StdinEof::Keep)?;
    }
    assert_eq!(
        admit(&mut a, 0, b"a", StdinEof::Keep)?,
        admit(&mut b, 0, b"a", StdinEof::Keep)?
    );
    Ok(())
}
