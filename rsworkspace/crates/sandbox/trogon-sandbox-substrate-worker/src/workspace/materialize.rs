use std::collections::BTreeMap;

use trogon_sandbox_contract::{
    CloneAuth, CommandEnv, CommandExecution, LaunchCancellation, LeaseId, RedactValues, RunCommandRequest,
    RunCommandResponse, StreamedOutput, WorkspaceSource,
};

use super::WorkspaceError;
use super::clone_url::authenticated_url;
use super::constants::CLONE_TIMEOUT_SECONDS;
use super::redact::redact;
use crate::agent::{ActorAgent, DiscardChunks};

/// Prepares `source` inside the actor. Must run after the lease is bound, since every
/// command the agent accepts is checked against the lease it is bound to.
pub async fn materialize(
    agent: &ActorAgent,
    lease_id: &LeaseId,
    source: &WorkspaceSource,
    working_dir: &str,
) -> Result<(), WorkspaceError> {
    match source {
        WorkspaceSource::Empty(_) => Ok(()),

        WorkspaceSource::Clone { url, sha, auth, .. } => {
            clone(agent, lease_id, url, sha, auth, working_dir).await
        }

        WorkspaceSource::HostMount { .. } => Err(WorkspaceError::Unsupported(
            "host_mount workspaces are not supported on the substrate provider: the worker has no access to the actor's node".to_string(),
        )),

        // The profile reports `supports_s3_backend: false`, and this is the call site that
        // makes that true rather than aspirational.
        WorkspaceSource::S3Volume { .. } => Err(WorkspaceError::Unsupported(
            "s3 volume workspaces are not supported on the substrate provider: an actor has no volume backend to sync against".to_string(),
        )),
    }
}

async fn clone(
    agent: &ActorAgent,
    lease_id: &LeaseId,
    url: &str,
    sha: &str,
    auth: &CloneAuth,
    working_dir: &str,
) -> Result<(), WorkspaceError> {
    let clone_url = authenticated_url(url, auth)?;

    // The URL carries the credential, so it goes in the environment rather than in argv:
    // argv is world-readable through `/proc` to every process in the sandbox, and a tenant
    // workload should not be able to lift the token out of a `ps` listing.
    let mut env = BTreeMap::new();
    env.insert("SANDBOX_CLONE_URL".to_string(), clone_url);
    env.insert("SANDBOX_CLONE_SHA".to_string(), sha.to_string());
    env.insert("SANDBOX_CLONE_DIR".to_string(), working_dir.to_string());

    let script = concat!(
        "set -e; ",
        "git clone --depth=1 \"$SANDBOX_CLONE_URL\" \"$SANDBOX_CLONE_DIR\"; ",
        // The shallow clone above may not contain the requested commit, so fetch it by name
        // before checking out. Without this a lease pinned to anything but the branch head
        // fails on an otherwise healthy repository.
        "git -C \"$SANDBOX_CLONE_DIR\" fetch --depth=1 origin \"$SANDBOX_CLONE_SHA\"; ",
        "git -C \"$SANDBOX_CLONE_DIR\" checkout \"$SANDBOX_CLONE_SHA\"",
    );

    let request = RunCommandRequest {
        require_launch_cancellation: LaunchCancellation::NotRequired,
        launch_key: None,
        lease_id: lease_id.clone(),
        command: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
        cwd: None,
        env: CommandEnv::new(env)?,
        timeout_seconds: CLONE_TIMEOUT_SECONDS,
        stream_output: StreamedOutput::Disabled,
        run_as_user: None,
        output_retention: Default::default(),
        max_output_bytes: None,
        redact_values: RedactValues::default(),
        execution: CommandExecution::Attached,
    };

    match agent.exec(&request, &DiscardChunks).await? {
        RunCommandResponse::Completed(output) if output.exit_code == 0 => Ok(()),
        RunCommandResponse::Completed(output) => Err(WorkspaceError::Failed(format!(
            "git clone exited {}: {}",
            output.exit_code,
            redact(&String::from_utf8_lossy(&output.stderr), url)
        ))),
        RunCommandResponse::Started(_) => Err(WorkspaceError::Failed(
            "the agent detached a workspace clone that was requested synchronously".to_string(),
        )),
        RunCommandResponse::Denied(error) => Err(WorkspaceError::Failed(format!("clone denied: {error:?}"))),
    }
}
