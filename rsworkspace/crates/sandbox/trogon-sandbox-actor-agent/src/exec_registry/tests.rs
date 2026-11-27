use std::error::Error;
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;

use super::*;
use crate::stdin_deadlines::StdinDeadlines;

fn id() -> ExecutionId {
    ExecutionId::new("exec-1")
}

fn request(seq: u64) -> Result<WriteStdinRequest, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "lease_id": "01J0",
        "execution_id": id().as_str(),
        "seq": seq,
        "data": [120],
        "eof": false,
    }))?)
}

async fn register(registry: &ExecRegistry, execution: Execution) {
    registry.executions.lock().await.insert(id(), Arc::new(execution));
}

fn quick() -> StdinDeadlines {
    StdinDeadlines {
        accept: Duration::from_millis(50),
        started: Duration::from_millis(50),
    }
}

#[tokio::test]
async fn killing_an_execution_whose_owner_is_gone_is_already_completed() {
    let registry = ExecRegistry::new();
    let (kill, receiver) = mpsc::channel(1);
    drop(receiver);
    register(
        &registry,
        Execution {
            state: Mutex::new(ExecutionState::Running),
            kill,
            stdin: None,
        },
    )
    .await;
    assert_eq!(
        registry.kill(&id(), UnixSignal::Kill).await,
        KillCommandResponse::AlreadyCompleted
    );
}

#[tokio::test]
async fn a_write_nobody_accepts_is_backpressure() -> Result<(), Box<dyn Error>> {
    let registry = ExecRegistry::new();
    let (kill, _kills) = mpsc::channel(1);
    let (handle, _ops) = StdinHandle::channel_with(1, quick());
    register(
        &registry,
        Execution {
            state: Mutex::new(ExecutionState::Running),
            kill,
            stdin: Some(handle),
        },
    )
    .await;
    let answer = registry.write_stdin(&request(0)?).await;
    assert!(
        matches!(answer, WriteStdinResponse::Backpressure { .. }),
        "got {answer:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_write_to_a_vanished_writer_breaks_stdin_for_good() -> Result<(), Box<dyn Error>> {
    let registry = ExecRegistry::new();
    let (kill, _kills) = mpsc::channel(1);
    let (handle, ops) = StdinHandle::channel_with(1, quick());
    drop(ops);
    register(
        &registry,
        Execution {
            state: Mutex::new(ExecutionState::Running),
            kill,
            stdin: Some(handle),
        },
    )
    .await;
    assert_eq!(
        registry.write_stdin(&request(0)?).await,
        WriteStdinResponse::StdinClosed
    );
    assert_eq!(
        registry.write_stdin(&request(1)?).await,
        WriteStdinResponse::StdinClosed
    );
    Ok(())
}
