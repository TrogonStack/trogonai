//! Named launches report whether they created or reused an execution, and strong launch requests are refused before
//! anything runs on a profile that cannot cancel a launch.

use std::time::Duration;

use anyhow::{Context, ensure};
use serde_json::{Value, json};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{ExecutionId, ListProcessesResponse};

use super::docker_lease::{docker_provider, node_lease_request};
use super::*;
use crate::fresh_id::fresh_id;

/// A provider holding one fresh node lease, plus the worker's view of that lease as active.
struct NamedLease {
    _server: JetStreamTestServer,
    client: Client,
    docker: bollard::Docker,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
    lease_id: LeaseId,
    sandbox_id: SandboxId,
}

impl NamedLease {
    async fn start(prefix: &str) -> anyhow::Result<Self> {
        let docker = bollard::Docker::connect_with_defaults()?;
        docker.info().await?;
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let owner = format!("{prefix}-{}", fresh_id());
        let provider = Arc::new(docker_provider(
            &client,
            format!("nats://{}", server.address()),
            &owner,
        )?);
        let lease_id = LeaseId::new(fresh_id());
        let request = node_lease_request(lease_id.as_str())?;
        let grant = provider.lease(&request, &lease_id).await?;
        let active = Arc::new(Mutex::new(HashMap::from([(
            lease_id.clone(),
            CancellationToken::new(),
        )])));
        Ok(Self {
            _server: server,
            client,
            docker,
            provider,
            active,
            lease_id,
            sandbox_id: grant.sandbox_id,
        })
    }

    /// The JSON reply the worker sends for one `run_command` payload.
    async fn launch_reply(&self, mut request: Value) -> anyhow::Result<Value> {
        if let Some(fields) = request.as_object_mut() {
            fields.insert("lease_id".into(), json!(self.lease_id.as_str()));
        }
        let inbox = self.client.new_inbox();
        let mut replies = self.client.subscribe(inbox.clone()).await?;
        self.client.flush().await?;
        let body = serde_json::to_vec(&request)?;
        let message = async_nats::Message {
            subject: RUN_COMMAND_SUBJECT.into(),
            reply: Some(inbox.into()),
            length: body.len(),
            payload: body.into(),
            headers: None,
            status: None,
            description: None,
        };
        handle_run_command_msg(message, self.client.clone(), self.provider.clone(), self.active.clone()).await?;
        let reply = tokio::time::timeout(Duration::from_secs(5), replies.next())
            .await?
            .context("named launch reply subscription closed")?;
        Ok(serde_json::from_slice(&reply.payload)?)
    }

    async fn launch_response(&self, request: Value) -> anyhow::Result<RunCommandResponse> {
        Ok(serde_json::from_value(self.launch_reply(request).await?)?)
    }

    async fn running(&self, execution_id: &str) -> bool {
        matches!(
            self.provider
                .poll_command_for_lease(&self.lease_id, &ExecutionId::new(execution_id.to_owned()))
                .await,
            Some(PollCommandResponse::Running)
        )
    }

    /// Cancels the lease and confirms the container is gone from the daemon.
    async fn close(&self) -> anyhow::Result<()> {
        let cleanup = self.provider.cancel(&self.lease_id).await;
        let absent = self.docker.inspect_container(self.sandbox_id.as_str(), None).await;
        cleanup?;
        ensure!(
            matches!(
                absent,
                Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
            ),
            "owned native container absence was not confirmed"
        );
        Ok(())
    }
}

fn named_sleeper(name: &str, launch_key: Option<&str>) -> Value {
    let mut request = json!({
        "command": ["sh", "-c", "sleep 60"],
        "detached": true,
        "deadline": "lease",
        "process": { "name": name },
    });
    if let (Some(fields), Some(key)) = (request.as_object_mut(), launch_key) {
        fields.insert("launch_key".into(), json!(key));
    }
    request
}

fn shell(script: &str) -> Value {
    json!({ "command": ["sh", "-c", script] })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_named_launch_replies_identify_exactly_one_creation() -> anyhow::Result<()> {
    let lease = NamedLease::start("named-origin").await?;
    let observed = async {
        let (first, second) = tokio::join!(
            lease.launch_reply(named_sleeper("web", Some("creating-request"))),
            lease.launch_reply(named_sleeper("web", Some("joining-request"))),
        );
        let replies = [first?, second?];
        for reply in &replies {
            ensure!(
                reply["outcome"] == "started",
                "detached named command was not acknowledged: {reply}"
            );
        }
        let first_id = replies[0]["execution_id"]
            .as_str()
            .context("first launch has no execution identity")?;
        let second_id = replies[1]["execution_id"]
            .as_str()
            .context("second launch has no execution identity")?;
        ensure!(
            first_id == second_id,
            "concurrent name reuse launched another execution"
        );
        let ListProcessesResponse::Processes { processes } =
            lease.provider.list_processes_for_lease(&lease.lease_id).await
        else {
            anyhow::bail!("owned provider process inventory was unavailable");
        };
        ensure!(processes.len() == 1, "named launch created duplicate processes");
        ensure!(
            lease.running(first_id).await,
            "the exact owned command must remain running before the origin assertion"
        );
        Ok::<_, anyhow::Error>(replies)
    }
    .await;
    lease.close().await?;
    let replies = observed?;
    let mut origins = replies.iter().map(|reply| reply["origin"].as_str()).collect::<Vec<_>>();
    origins.sort();
    ensure!(
        origins == vec![Some("created"), Some("reused")],
        "native named create/reuse replies lost their atomic origin: {origins:?}; same-execution, one-process and owned container absence controls passed"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_launch_cancellation_refuses_commands_before_native_effects() -> anyhow::Result<()> {
    ensure!(
        !crate::PROFILE
            .isolation
            .supports_command_launch_cancellation
            .is_supported(),
        "this refusal regression requires the actual unsupported Docker profile"
    );
    let lease = NamedLease::start("launch-refusal").await?;
    let observed = async {
        let sentinel = lease.launch_reply(named_sleeper("sentinel", None)).await?;
        let sentinel_id = sentinel["execution_id"]
            .as_str()
            .context("ordinary named sentinel was not started")?;
        let ordinary = lease
            .launch_response(shell("printf control > /tmp/ordinary-control"))
            .await?;
        ensure!(
            matches!(ordinary, RunCommandResponse::Completed(ref output) if output.exit_code == 0),
            "ordinary handler command did not execute: {ordinary:?}"
        );
        let foreground = lease
            .launch_response(json!({
                "command": ["sh", "-c", "printf forbidden > /tmp/strong-foreground"],
                "require_launch_cancellation": true,
                "launch_key": "strong-foreground",
            }))
            .await?;
        let detached = lease
            .launch_response(json!({
                "command": ["sh", "-c", "printf forbidden > /tmp/strong-detached; sleep 60"],
                "require_launch_cancellation": true,
                "launch_key": "strong-detached",
                "detached": true,
                "deadline": "lease",
                "process": { "name": "forbidden" },
            }))
            .await?;
        let processes = lease.provider.list_processes_for_lease(&lease.lease_id).await;
        ensure!(
            lease.running(sentinel_id).await,
            "the unrelated ordinary sentinel must survive the strong requests"
        );
        let markers = lease
            .launch_response(shell(
                "cat /tmp/ordinary-control; test ! -e /tmp/strong-foreground && test ! -e /tmp/strong-detached",
            ))
            .await?;
        Ok::<_, anyhow::Error>((foreground, detached, processes, markers))
    }
    .await;
    lease.close().await?;
    let (foreground, detached, processes, markers) = observed?;
    let refused = |response: &RunCommandResponse| matches!(response, RunCommandResponse::Denied(LeaseError::InvalidSpec { reason }) if reason.contains("command launch cancellation"));
    let no_registration = matches!(
        processes,
        ListProcessesResponse::Processes { ref processes }
            if processes.len() == 1
                && processes[0].name.as_ref().map(|name| name.as_str()) == Some("sentinel")
    );
    let no_markers = matches!(
        markers,
        RunCommandResponse::Completed(ref output) if output.exit_code == 0 && output.stdout == b"control"
    );
    ensure!(
        refused(&foreground) && refused(&detached) && no_registration && no_markers,
        "unsupported strong requests must be typed refusals with no native effects: foreground={foreground:?}, detached={detached:?}, processes={processes:?}, markers={markers:?}; ordinary execution, sentinel liveness and exact native absence controls passed"
    );
    Ok(())
}
