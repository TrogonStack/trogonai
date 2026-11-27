use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::StreamExt;
use trogon_sandbox_contract::{Base64Data, TerminalOutput};

use super::Session;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn data(seq: u64, text: &str) -> Result<TerminalOutput, Box<dyn std::error::Error>> {
    Ok(TerminalOutput::Data {
        seq,
        data: Base64Data::new(STANDARD.encode(text))?,
    })
}

#[tokio::test]
async fn a_read_waits_for_output_and_a_closed_stream_ends_the_session() -> TestResult {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    let session = Session::start(rx.boxed());
    let empty = session.read(10, Duration::from_millis(10)).await;
    assert_eq!(empty.output, "");
    assert!(!empty.exited);

    let reader = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.read(100, Duration::from_secs(5)).await })
    };
    tx.unbounded_send(data(0, "late")?)?;
    assert_eq!(reader.await?.output, "late");

    drop(tx);
    let ended = session.read(100, Duration::from_secs(5)).await;
    assert!(ended.exited);
    assert_eq!(ended.exit_code, None);
    Ok(())
}

#[tokio::test]
async fn an_exit_frame_stops_the_pump() -> TestResult {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    let session = Session::start(rx.boxed());
    tx.unbounded_send(TerminalOutput::Exit {
        seq: 0,
        exit_code: Some(3),
    })?;
    let ended = session.read(100, Duration::from_secs(5)).await;
    assert!(ended.exited);
    assert_eq!(ended.exit_code, Some(3));
    Ok(())
}

#[tokio::test]
async fn a_dropped_session_stops_filling_its_buffer() -> TestResult {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    let session = Session::start(rx.boxed());
    drop(session);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(tx.unbounded_send(data(0, "x")?).is_err());
    Ok(())
}
