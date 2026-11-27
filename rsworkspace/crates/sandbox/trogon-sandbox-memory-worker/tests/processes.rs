mod common;
mod nats_common;

use common::{Harness, TestResult, start};
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    ExecutionId, ListProcessesResponse, ProcessState, RunCommandResponse, WaitReadyResponse,
};

async fn launch(
    h: &Harness,
    command: Value,
    process: Value,
) -> Result<(ExecutionId, String), Box<dyn std::error::Error>> {
    let reply: RunCommandResponse = h
        .call(
            "run_command",
            "l1",
            json!({"lease_id": "l1", "command": command, "detached": true, "process": process}),
        )
        .await?;
    match reply {
        RunCommandResponse::Started(s) => Ok((s.execution_id, format!("{:?}", s.origin))),
        other => Err(format!("not started: {other:?}").into()),
    }
}

async fn wait(h: &Harness, id: &ExecutionId, timeout: u32) -> Result<WaitReadyResponse, Box<dyn std::error::Error>> {
    h.call(
        "wait_ready",
        "l1",
        json!({"lease_id": "l1", "execution_id": id, "timeout_seconds": timeout}),
    )
    .await
}

#[tokio::test]
async fn named_processes_report_readiness() -> TestResult {
    let h = start().await?;
    h.grant("l1").await?;

    let (web, origin) = launch(
        &h,
        json!(["serve", "8080"]),
        json!({"name": "web", "readiness": {"kind": "tcp_port", "port": 8080}}),
    )
    .await?;
    assert_eq!(origin, "Created");
    let (same, origin) = launch(&h, json!(["serve", "8080"]), json!({"name": "web"})).await?;
    assert_eq!((same, origin.as_str()), (web.clone(), "Reused"));
    assert!(matches!(wait(&h, &web, 5).await?, WaitReadyResponse::Ready));

    let (silent, _) = launch(
        &h,
        json!(["serve"]),
        json!({"name": "silent", "readiness": {"kind": "tcp_port", "port": 9}}),
    )
    .await?;
    assert!(matches!(wait(&h, &silent, 0).await?, WaitReadyResponse::NotReady));

    let (log, _) = launch(
        &h,
        json!(["serve"]),
        json!({"name": "log", "readiness": {"kind": "output_contains", "needle": "listening"}}),
    )
    .await?;
    assert!(matches!(wait(&h, &log, 5).await?, WaitReadyResponse::Ready));

    let (plain, _) = launch(&h, json!(["serve"]), json!({"name": "plain"})).await?;
    assert!(matches!(wait(&h, &plain, 1).await?, WaitReadyResponse::NoProbe));
    assert!(matches!(
        wait(&h, &ExecutionId::new("nope"), 1).await?,
        WaitReadyResponse::NotFound
    ));

    let (failing, _) = launch(
        &h,
        json!(["sh", "-c", "exit 3"]),
        json!({"readiness": {"kind": "output_contains", "needle": "never"}}),
    )
    .await?;
    loop {
        match wait(&h, &failing, 1).await? {
            WaitReadyResponse::Failed { exit_code } => {
                assert_eq!(exit_code, 3);
                break;
            }
            WaitReadyResponse::NotReady => {}
            other => return Err(format!("unexpected {other:?}").into()),
        }
    }
    let (talker, _) = launch(
        &h,
        json!(["echo", "ready now"]),
        json!({"readiness": {"kind": "output_contains", "needle": "ready now"}}),
    )
    .await?;
    loop {
        if matches!(wait(&h, &talker, 1).await?, WaitReadyResponse::Ready) {
            break;
        }
    }
    let (_exited, _) = launch(&h, json!(["true"]), json!({"name": "web2"})).await?;
    let (_again, origin) = launch(&h, json!(["true"]), json!({"name": "web2"})).await?;
    assert!(origin == "Created" || origin == "Reused");

    let ListProcessesResponse::Processes { processes } =
        h.call("list_processes", "l1", json!({"lease_id": "l1"})).await?
    else {
        return Err("not a process list".into());
    };
    assert!(processes.iter().any(|p| p.state == ProcessState::Running));
    assert!(
        processes
            .windows(2)
            .all(|pair| pair[0].execution_id <= pair[1].execution_id)
    );
    h.worker.shutdown().await;
    Ok(())
}
