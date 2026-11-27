mod common;
mod nats_common;

use common::{TestResult, start};
use serde_json::json;
use trogon_sandbox_contract::{
    CommandTimeoutStatus, KillCommandResponse, PollCommandResponse, RunCommandResponse, StopCommandResponse,
};

#[tokio::test]
async fn attached_commands_run_to_completion() -> TestResult {
    let h = start().await?;
    h.grant("l1").await?;
    h.call_raw(
        "write_files",
        "l1",
        json!({"lease_id": "l1", "files": [{"path": "/workspace/a.txt", "content": [104, 105]}]}),
    )
    .await?;
    let run = |command: serde_json::Value, extra: serde_json::Value| {
        let mut body = json!({"lease_id": "l1", "command": command});
        if let (Some(map), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            map.extend(extra.clone());
        }
        body
    };
    let echo: RunCommandResponse = h
        .call("run_command", "l1", run(json!(["echo", "a", "b"]), json!({})))
        .await?;
    assert!(matches!(&echo, RunCommandResponse::Completed(o) if o.stdout == b"a b\n" && o.exit_code == 0));
    let cat: RunCommandResponse = h
        .call(
            "run_command",
            "l1",
            run(json!(["cat", "a.txt"]), json!({"cwd": "/workspace"})),
        )
        .await?;
    assert!(matches!(&cat, RunCommandResponse::Completed(o) if o.stdout == b"hi"));
    let missing: RunCommandResponse = h
        .call("run_command", "l1", run(json!(["true"]), json!({"cwd": "/nope"})))
        .await?;
    assert!(
        matches!(&missing, RunCommandResponse::Completed(o) if o.exit_code == 1 && o.stderr.starts_with(b"cd: /nope"))
    );
    let signalled: RunCommandResponse = h
        .call(
            "run_command",
            "l1",
            run(json!(["sleep", "30"]), json!({"timeout_seconds": 1})),
        )
        .await?;
    assert!(matches!(&signalled, RunCommandResponse::Completed(o) if o.timed_out == CommandTimeoutStatus::TimedOut));
    let redacted: RunCommandResponse = h
        .call(
            "run_command",
            "l1",
            run(
                json!(["echo", "hunter2-literal"]),
                json!({"redact_values": ["hunter2-literal"]}),
            ),
        )
        .await?;
    assert!(
        matches!(&redacted, RunCommandResponse::Completed(o) if !String::from_utf8_lossy(&o.stdout).contains("hunter2"))
    );
    for denied in [
        json!({"require_launch_cancellation": true}),
        json!({"detached": true, "stdin": "interactive"}),
        json!({"detached": true, "deadline": "lease"}),
        json!({"stdin": "interactive"}),
    ] {
        let reply: RunCommandResponse = h.call("run_command", "l1", run(json!(["true"]), denied)).await?;
        assert!(matches!(reply, RunCommandResponse::Denied(_)));
    }
    h.worker.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn detached_commands_are_polled_signalled_and_restarted() -> TestResult {
    let h = start().await?;
    h.grant("l1").await?;
    let started = |reply: RunCommandResponse| match reply {
        RunCommandResponse::Started(s) => Ok(s.execution_id),
        other => Err(format!("not started: {other:?}")),
    };
    let quick = started(
        h.call(
            "run_command",
            "l1",
            json!({"lease_id": "l1", "command": ["echo", "x"], "detached": true}),
        )
        .await?,
    )?;
    let poll = json!({"lease_id": "l1", "execution_id": quick});
    loop {
        let reply: PollCommandResponse = h.call("poll_command", "l1", poll.clone()).await?;
        if matches!(reply, PollCommandResponse::Completed(_)) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let again: KillCommandResponse = h.call("kill_command", "l1", poll.clone()).await?;
    assert!(matches!(again, KillCommandResponse::AlreadyCompleted));
    let stop: StopCommandResponse = h.call("stop_command", "l1", poll.clone()).await?;
    assert!(matches!(stop, StopCommandResponse::AlreadyCompleted));
    let unknown = json!({"lease_id": "l1", "execution_id": "nope"});
    assert!(matches!(
        h.call::<PollCommandResponse>("poll_command", "l1", unknown.clone())
            .await?,
        PollCommandResponse::NotFound
    ));
    assert!(matches!(
        h.call::<KillCommandResponse>("kill_command", "l1", unknown.clone())
            .await?,
        KillCommandResponse::NotFound
    ));
    assert!(matches!(
        h.call::<StopCommandResponse>("stop_command", "l1", unknown.clone())
            .await?,
        StopCommandResponse::NotFound
    ));

    let sleeper = started(
        h.call(
            "run_command",
            "l1",
            json!({"lease_id": "l1", "command": ["sleep", "60"], "detached": true}),
        )
        .await?,
    )?;
    let sleeper_ref = json!({"lease_id": "l1", "execution_id": sleeper});
    assert!(matches!(
        h.call::<PollCommandResponse>("poll_command", "l1", sleeper_ref.clone())
            .await?,
        PollCommandResponse::Running
    ));
    let signalled = json!({"lease_id": "l1", "execution_id": sleeper, "signal": "term"});
    assert!(matches!(
        h.call::<KillCommandResponse>("kill_command", "l1", signalled).await?,
        KillCommandResponse::Signaled
    ));
    loop {
        if let PollCommandResponse::Completed(o) = h.call("poll_command", "l1", sleeper_ref.clone()).await? {
            assert_eq!(o.exit_code, 128 + 15);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let victim = started(
        h.call(
            "run_command",
            "l1",
            json!({"lease_id": "l1", "command": ["sleep", "60"], "detached": true}),
        )
        .await?,
    )?;
    let kill = json!({"lease_id": "l1", "execution_id": victim, "signal": "kill"});
    assert!(matches!(
        h.call::<KillCommandResponse>("kill_command", "l1", kill).await?,
        KillCommandResponse::Signaled
    ));
    let reply: PollCommandResponse = h
        .call("poll_command", "l1", json!({"lease_id": "l1", "execution_id": victim}))
        .await?;
    assert!(matches!(reply, PollCommandResponse::Completed(o) if o.exit_code == 137));

    let restart_target = started(
        h.call(
            "run_command",
            "l1",
            json!({"lease_id": "l1", "command": ["sleep", "60"], "detached": true}),
        )
        .await?,
    )?;
    let restarted: serde_json::Value = h
        .call_raw(
            "restart_command",
            "l1",
            json!({"lease_id": "l1", "execution_id": restart_target, "grace_seconds": 5}),
        )
        .await?;
    assert!(restarted.to_string().contains("execution_id"));
    let missing: serde_json::Value = h
        .call_raw(
            "restart_command",
            "l1",
            json!({"lease_id": "l1", "execution_id": "nope"}),
        )
        .await?;
    assert!(missing.to_string().contains("not_found"));

    let stubborn = started(
        h.call(
            "run_command",
            "l1",
            json!({"lease_id": "l1", "command": ["sleep", "60"], "detached": true}),
        )
        .await?,
    )?;
    let stopped: StopCommandResponse = h
        .call(
            "stop_command",
            "l1",
            json!({"lease_id": "l1", "execution_id": stubborn, "grace_seconds": 1000}),
        )
        .await?;
    assert!(matches!(stopped, StopCommandResponse::Stopped { .. }));
    h.worker.shutdown().await;
    Ok(())
}
