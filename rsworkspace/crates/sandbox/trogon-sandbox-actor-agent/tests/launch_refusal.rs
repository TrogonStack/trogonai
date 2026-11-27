#![cfg(feature = "server")]

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, Response};
use serde::Serialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_actor_agent::{
    ActorIdentitySource, AgentState, BIND_PATH, EXEC_KILL_PATH, EXEC_PATH, EXEC_POLL_PATH, ExecFrame,
    GUEST_PROTOCOL_VERSION, WORKER_PROTOCOL_VERSION_HEADER, router, serve,
};
use trogon_sandbox_contract::{KillCommandResponse, LeaseError, PollCommandResponse, RunCommandResponse};

type TestResult = Result<(), Box<dyn Error>>;

struct Agent {
    endpoint: String,
    client: Client,
    cancel: CancellationToken,
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Agent {
    async fn post<T: Serialize>(&self, path: &str, body: &T) -> Result<Response, Box<dyn Error>> {
        let request = self.client.post(format!("{}{path}", self.endpoint));
        let request = request.header(WORKER_PROTOCOL_VERSION_HEADER, GUEST_PROTOCOL_VERSION);
        Ok(request.json(body).send().await?.error_for_status()?)
    }

    async fn run(&self, request: &Value) -> Result<Vec<ExecFrame>, Box<dyn Error>> {
        let body = self.post(EXEC_PATH, request).await?.text().await?;
        body.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).map_err(Into::into))
            .collect()
    }

    async fn poll(&self, lease: &str, execution: &str) -> Result<PollCommandResponse, Box<dyn Error>> {
        let body = json!({"lease_id": lease, "execution_id": execution});
        Ok(self.post(EXEC_POLL_PATH, &body).await?.json().await?)
    }

    async fn cleanup(&self, lease: &str, execution: &str) -> TestResult {
        let body = json!({"lease_id": lease, "execution_id": execution, "signal": "kill"});
        let killed: KillCommandResponse = self.post(EXEC_KILL_PATH, &body).await?.json().await?;
        assert!(
            matches!(
                killed,
                KillCommandResponse::Signaled | KillCommandResponse::AlreadyCompleted
            ),
            "owned native command must accept exact cleanup: {killed:?}"
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if matches!(self.poll(lease, execution).await?, PollCommandResponse::Completed(_)) {
                    return Ok::<_, Box<dyn Error>>(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        Ok(())
    }
}

fn started(frames: &[ExecFrame]) -> Option<String> {
    frames.iter().find_map(|frame| match frame {
        ExecFrame::Response(RunCommandResponse::Started(started)) => Some(started.execution_id.as_str().to_string()),
        _ => None,
    })
}

fn completed(frames: &[ExecFrame]) -> bool {
    frames.iter().any(
        |frame| matches!(frame, ExecFrame::Response(RunCommandResponse::Completed(output)) if output.exit_code == 0),
    )
}

fn refused(frames: &[ExecFrame]) -> bool {
    matches!(
        frames,
        [ExecFrame::Response(RunCommandResponse::Denied(LeaseError::InvalidSpec { reason }))]
            if reason.contains("command launch cancellation")
    )
}

#[tokio::test]
async fn unsupported_launch_cancellation_is_refused_before_native_effects() -> TestResult {
    let root = tempfile::tempdir()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let cancel = CancellationToken::new();
    let state = Arc::new(AgentState::new(ActorIdentitySource::new(root.path().join("identity"))));
    tokio::spawn(serve(listener, router(state), cancel.clone()));
    let agent = Agent {
        endpoint,
        client: Client::new(),
        cancel,
    };
    let lease = "owned-launch-refusal";
    let working_dir = root.path().to_string_lossy().to_string();
    let bound = agent
        .post(BIND_PATH, &json!({"lease_id": lease, "working_dir": working_dir}))
        .await?;
    assert_eq!(bound.status(), 204);

    let mut owned = Vec::new();
    let observed = async {
        let sentinel = json!({
            "lease_id": lease,
            "command": ["/bin/sh", "-c", "printf sentinel > sentinel; exec sleep 60"],
            "detached": true,
            "timeout_seconds": 60,
        });
        let frames = agent.run(&sentinel).await?;
        let sentinel_id = started(&frames).ok_or("ordinary native sentinel was not started")?;
        owned.push(sentinel_id.clone());

        let ordinary = json!({
            "lease_id": lease,
            "command": ["/bin/sh", "-c", "printf control > control"],
            "timeout_seconds": 1,
        });
        let control = agent.run(&ordinary).await?;
        assert!(completed(&control), "ordinary command must complete: {control:?}");
        assert_eq!(tokio::fs::read(root.path().join("control")).await?, b"control");

        let foreground = json!({
            "lease_id": lease,
            "command": ["/bin/sh", "-c", "printf forbidden > foreground"],
            "timeout_seconds": 1,
            "require_launch_cancellation": true,
            "launch_key": "strong-foreground",
            "stream_output": true,
        });
        let foreground = agent.run(&foreground).await?;
        let detached = json!({
            "lease_id": lease,
            "command": ["/bin/sh", "-c", "printf forbidden > detached; exec sleep 60"],
            "timeout_seconds": 60,
            "detached": true,
            "require_launch_cancellation": true,
            "launch_key": "strong-detached",
        });
        let detached = agent.run(&detached).await?;
        owned.extend(started(&detached));
        let sentinel_running = matches!(agent.poll(lease, &sentinel_id).await?, PollCommandResponse::Running);
        let no_native_markers = !root.path().join("foreground").exists() && !root.path().join("detached").exists();
        Ok::<_, Box<dyn Error>>((foreground, detached, sentinel_running, no_native_markers))
    }
    .await;
    for execution in owned {
        agent.cleanup(lease, &execution).await?;
    }
    let (foreground, detached, sentinel_running, no_native_markers) = observed?;
    assert!(refused(&foreground), "foreground: {foreground:?}");
    assert!(refused(&detached), "detached: {detached:?}");
    assert!(sentinel_running);
    assert!(no_native_markers);
    Ok(())
}
