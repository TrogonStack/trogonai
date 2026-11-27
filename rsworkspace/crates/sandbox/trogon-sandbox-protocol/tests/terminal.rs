use std::time::{Duration, Instant};
use trogon_std::env::SystemEnv;

use trogon_sandbox_contract::{TerminalInput, TerminalOutput};
use trogon_sandbox_protocol::{
    ActivitySignal, MAX_TERMINAL_OUTPUT_CHUNK_BYTES, MAX_TERMINALS_PER_LEASE, TERMINAL_REPLAY_BYTES_ENV,
    TerminalReplayRing, terminal_data_frame, terminal_input_bytes, terminal_output_bytes, terminal_output_seq,
    terminal_replay_bytes_from_env, terminal_replay_bytes_from_value,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn seqs(frames: &[TerminalOutput]) -> Vec<u64> {
    frames.iter().map(terminal_output_seq).collect()
}

fn first_bytes(frames: &[TerminalOutput]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let frame = frames.first().ok_or("no frame")?;
    terminal_output_bytes(frame).ok_or_else(|| "frame carries no bytes".into())
}

#[test]
fn replay_ring_keeps_the_latest_bytes_and_original_seqs() -> TestResult {
    let mut ring = TerminalReplayRing::new(8);
    ring.push(0, b"abcd");
    ring.push(1, b"efgh");
    ring.push(2, b"ij");
    assert_eq!(ring.len(), 6);
    assert!(!ring.is_empty());
    let frames = ring.replay_frames()?;
    assert_eq!(seqs(&frames), vec![1, 2]);
    assert_eq!(first_bytes(&frames)?, b"efgh");
    Ok(())
}

#[test]
fn replay_ring_ignores_empty_pushes() -> TestResult {
    let mut ring = TerminalReplayRing::new(8);
    ring.push(0, b"");
    assert!(ring.is_empty());
    assert!(ring.reattach_frames(Some(0))?.is_empty());
    Ok(())
}

#[test]
fn replay_ring_trims_an_oversized_frame_to_its_tail() -> TestResult {
    let mut ring = TerminalReplayRing::new(4);
    ring.push(7, b"0123456789");
    let frames = ring.replay_frames()?;
    assert_eq!(frames.len(), 1);
    assert_eq!(seqs(&frames), vec![7]);
    assert_eq!(first_bytes(&frames)?, b"6789");
    Ok(())
}

#[test]
fn replay_ring_of_zero_is_off_and_capacity_is_clamped() -> TestResult {
    let mut off = TerminalReplayRing::new(0);
    off.push(0, b"secret");
    assert!(off.is_empty());
    assert!(off.replay_frames()?.is_empty());
    assert_eq!(
        TerminalReplayRing::new(usize::MAX).capacity(),
        TerminalReplayRing::MAX_CAPACITY
    );
    assert_eq!(
        TerminalReplayRing::with_default_capacity().capacity(),
        TerminalReplayRing::DEFAULT_CAPACITY
    );
    Ok(())
}

#[test]
fn reattach_resumes_after_the_offset_when_the_ring_covers_it() -> TestResult {
    let mut ring = TerminalReplayRing::new(100);
    ring.push(0, b"a");
    ring.push(1, b"b");
    ring.push(2, b"c");
    assert_eq!(seqs(&ring.reattach_frames(Some(0))?), vec![1, 2]);
    assert!(ring.reattach_frames(Some(2))?.is_empty());
    assert_eq!(seqs(&ring.reattach_frames(None)?), vec![0, 1, 2]);
    Ok(())
}

#[test]
fn reattach_asks_for_a_resync_when_the_offset_fell_out_of_the_ring() -> TestResult {
    let mut ring = TerminalReplayRing::new(2);
    ring.push(0, b"a");
    ring.push(1, b"b");
    ring.push(2, b"c");
    ring.push(3, b"d");
    let frames = ring.reattach_frames(Some(0))?;
    assert_eq!(frames.first(), Some(&TerminalOutput::Resync { from_seq: 2 }));
    assert_eq!(seqs(frames.get(1..).ok_or("missing replay")?), vec![2, 3]);
    assert_eq!(seqs(&ring.reattach_frames(Some(1))?), vec![2, 3]);
    Ok(())
}

#[test]
fn reattach_resyncs_over_a_trimmed_frame_and_when_replay_is_off() -> TestResult {
    let mut ring = TerminalReplayRing::new(4);
    ring.push(5, b"0123456789");
    assert_eq!(
        ring.reattach_frames(Some(4))?.first(),
        Some(&TerminalOutput::Resync { from_seq: 5 })
    );
    assert!(ring.reattach_frames(Some(5))?.is_empty());

    let mut off = TerminalReplayRing::new(0);
    off.push(0, b"x");
    off.push(1, b"y");
    assert_eq!(
        off.reattach_frames(Some(0))?,
        vec![TerminalOutput::Resync { from_seq: 2 }]
    );
    assert!(off.reattach_frames(Some(1))?.is_empty());
    Ok(())
}

#[test]
fn an_empty_ring_replays_nothing() -> TestResult {
    let ring = TerminalReplayRing::new(4);
    assert!(ring.reattach_frames(Some(0))?.is_empty());
    assert!(ring.reattach_frames(None)?.is_empty());
    Ok(())
}

#[test]
fn replay_ring_tracks_the_newest_seq_when_pushed_out_of_order() -> TestResult {
    let mut ring = TerminalReplayRing::new(0);
    ring.push(5, b"x");
    ring.push(3, b"y");
    assert_eq!(
        ring.reattach_frames(Some(0))?,
        vec![TerminalOutput::Resync { from_seq: 6 }]
    );
    Ok(())
}

#[test]
fn data_frames_roundtrip_through_the_wire_encoding() -> TestResult {
    let frame = terminal_data_frame(3, b"hi\r\n")?;
    assert_eq!(
        serde_json::to_string(&frame)?,
        r#"{"type":"data","seq":3,"data":"aGkNCg=="}"#
    );
    assert_eq!(terminal_output_seq(&frame), 3);
    assert_eq!(terminal_output_bytes(&frame), Some(b"hi\r\n".to_vec()));
    let back: TerminalOutput = serde_json::from_str(&serde_json::to_string(&frame)?)?;
    assert_eq!(back, frame);
    Ok(())
}

#[test]
fn only_data_frames_carry_bytes() {
    let exit = TerminalOutput::Exit {
        seq: 4,
        exit_code: Some(0),
    };
    assert_eq!(terminal_output_seq(&exit), 4);
    assert_eq!(terminal_output_bytes(&exit), None);
    let resync = TerminalOutput::Resync { from_seq: 9 };
    assert_eq!(terminal_output_seq(&resync), 9);
    assert_eq!(terminal_output_bytes(&resync), None);
}

#[test]
fn input_writes_decode_to_their_bytes() -> TestResult {
    let input = TerminalInput::new("bHMK")?;
    assert_eq!(terminal_input_bytes(&input)?, b"ls\n");
    let empty = TerminalInput::new("")?;
    assert!(terminal_input_bytes(&empty)?.is_empty());
    Ok(())
}

#[test]
fn activity_signal_is_due_once_per_interval() {
    let signal = ActivitySignal::default();
    let start = Instant::now();
    assert!(signal.due(start));
    assert!(!signal.due(start + Duration::from_secs(1)));
    assert!(signal.due(start + ActivitySignal::INTERVAL));
    assert!(!signal.due(start + ActivitySignal::INTERVAL));
}

#[test]
fn activity_signal_tolerates_a_clock_that_moves_backwards() {
    let signal = ActivitySignal::default();
    let start = Instant::now();
    assert!(signal.due(start + Duration::from_secs(10)));
    assert!(!signal.due(start));
}

#[test]
fn replay_size_comes_from_the_configuration_value() {
    assert_eq!(
        terminal_replay_bytes_from_value(None),
        TerminalReplayRing::DEFAULT_CAPACITY
    );
    assert_eq!(
        terminal_replay_bytes_from_value(Some("not a number")),
        TerminalReplayRing::DEFAULT_CAPACITY
    );
    assert_eq!(terminal_replay_bytes_from_value(Some(" 0 ")), 0);
    assert_eq!(terminal_replay_bytes_from_value(Some("4096")), 4096);
    assert_eq!(
        terminal_replay_bytes_from_value(Some("999999999")),
        TerminalReplayRing::MAX_CAPACITY
    );
}

#[test]
fn replay_size_from_the_environment_stays_in_range() {
    assert_eq!(TERMINAL_REPLAY_BYTES_ENV, "SANDBOX_TERMINAL_REPLAY_BYTES");
    assert!(terminal_replay_bytes_from_env(&SystemEnv) <= TerminalReplayRing::MAX_CAPACITY);
}

#[test]
fn limits_keep_a_frame_below_the_payload_ceiling() {
    assert_eq!(MAX_TERMINALS_PER_LEASE, 8);
    const { assert!(MAX_TERMINAL_OUTPUT_CHUNK_BYTES < TerminalInput::MAX_BYTES) };
}
