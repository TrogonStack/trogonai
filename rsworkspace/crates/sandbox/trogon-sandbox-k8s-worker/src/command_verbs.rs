use std::sync::Arc;
use std::time::Duration;

use async_nats::Message;
use trogon_sandbox_contract::{
    CommandEnv, CommandExecution, CommandOutput, ExecutionId, KillCommandRequest, LeaseError, PollCommandRequest,
    PollCommandResponse, RestartCommandRequest, RunCommandRequest, RunCommandResponse, RunCommandStartedResponse,
    StopCommandRequest, WriteStdinRequest,
};
use trogon_sandbox_protocol::{
    effective_max_output_bytes, fit_poll_command_response, fit_run_command_response, resolve_exec_cwd,
};
use trogon_sandbox_providers::PodExecOutput;

use crate::inbound::Inbound;
use crate::run_command_violation_error::RunCommandViolationError;
use crate::shell_wrap::shell_wrap;
use crate::verb_context::VerbContext;
use crate::{CommandBuilder, K8sVerb, PollOutcome};

fn output_of(output: PodExecOutput) -> CommandOutput {
    CommandOutput {
        exit_code: output.exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
        timed_out: output.timed_out.into(),
        truncated: output.truncated,
    }
}

/// The argv of a command for one execution id, which it puts in the command's environment as the signal marker.
fn command_builder(request: &RunCommandRequest) -> CommandBuilder {
    let command = request.command.clone();
    let env = request.env.as_map().clone();
    let user = request.run_as_user.clone();
    // With no user the shell starts in the container's working dir, so a relative cwd already resolves against it;
    // a user's default is its home.
    let cwd = match user.as_ref() {
        Some(user) => Some(resolve_exec_cwd(
            &format!("/home/{}", user.as_str()),
            request.cwd.as_deref(),
        )),
        None => request.cwd.clone(),
    };
    Arc::new(move |execution_id: &str| {
        let mut vars = env.clone();
        vars.insert(CommandEnv::RESERVED_KEY.to_owned(), execution_id.to_owned());
        shell_wrap(&command, cwd.as_deref(), &vars, user.as_ref().map(|user| user.as_str()))
    })
}

pub(crate) async fn run_command(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<RunCommandRequest>::decode(&msg, K8sVerb::RunCommand) else {
        return;
    };
    let (reply, request) = (inbound.reply, inbound.request);
    if let Some(violation) = RunCommandViolationError::of(&request) {
        if ctx.provider.holds_lease(&request.lease_id).await {
            let denied = LeaseError::InvalidSpec {
                reason: violation.to_string(),
            };
            ctx.reply(reply, &RunCommandResponse::Denied(denied)).await;
        }
        return;
    }
    let build = command_builder(&request);
    let timeout = Duration::from_secs(u64::from(request.timeout_seconds));
    let max_output_bytes = effective_max_output_bytes(request.max_output_bytes, usize::MAX);
    let response = match &request.execution {
        CommandExecution::Detached { stdin, .. } => {
            let started = ctx
                .provider
                .start_detached_exec_for_lease(
                    &request.lease_id,
                    build,
                    *stdin,
                    request.output_retention,
                    timeout,
                    max_output_bytes,
                )
                .await;
            started.map(|execution_id| {
                RunCommandResponse::Started(RunCommandStartedResponse {
                    origin: Default::default(),
                    execution_id: ExecutionId::new(execution_id),
                })
            })
        }
        CommandExecution::Attached => {
            let result = ctx
                .provider
                .exec_bounded_for_lease(&request.lease_id, |id| build(id), timeout, max_output_bytes)
                .await;
            result.map(|result| match result {
                Ok(output) => {
                    fit_run_command_response(RunCommandResponse::Completed(output_of(output)), ctx.max_payload())
                }
                Err(error) => RunCommandResponse::Denied(LeaseError::Backend {
                    message: error.to_string(),
                }),
            })
        }
    };
    if let Some(response) = response {
        ctx.reply(reply, &response).await;
    }
}

pub(crate) async fn poll_command(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<PollCommandRequest>::decode(&msg, K8sVerb::PollCommand) else {
        return;
    };
    let request = inbound.request;
    let outcome = ctx
        .provider
        .poll_command_execution(&request.lease_id, request.execution_id.as_str())
        .await;
    let Some(outcome) = outcome else { return };
    let response = match outcome {
        PollOutcome::Running => PollCommandResponse::Running,
        PollOutcome::Completed(output) => PollCommandResponse::Completed(output_of(output)),
        PollOutcome::Failed(message) => PollCommandResponse::Denied(LeaseError::Backend { message }),
        PollOutcome::NotFound => PollCommandResponse::NotFound,
    };
    ctx.reply(inbound.reply, &fit_poll_command_response(response, ctx.max_payload()))
        .await;
}

pub(crate) async fn kill_command(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<KillCommandRequest>::decode(&msg, K8sVerb::KillCommand) else {
        return;
    };
    let request = inbound.request;
    let response = ctx
        .provider
        .kill_command_for_lease(&request.lease_id, request.execution_id.as_str(), request.signal)
        .await;
    if let Some(response) = response {
        ctx.reply(inbound.reply, &response).await;
    }
}

fn grace(seconds: u32) -> Duration {
    Duration::from_secs(u64::from(seconds.min(StopCommandRequest::MAX_GRACE_SECONDS)))
}

pub(crate) async fn stop_command(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<StopCommandRequest>::decode(&msg, K8sVerb::StopCommand) else {
        return;
    };
    let request = inbound.request;
    let response = ctx
        .provider
        .stop_command_for_lease(
            &request.lease_id,
            request.execution_id.as_str(),
            grace(request.grace_seconds),
        )
        .await;
    if let Some(response) = response {
        ctx.reply(inbound.reply, &response).await;
    }
}

pub(crate) async fn restart_command(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<RestartCommandRequest>::decode(&msg, K8sVerb::RestartCommand) else {
        return;
    };
    let request = inbound.request;
    let response = ctx
        .provider
        .restart_command_for_lease(
            &request.lease_id,
            request.execution_id.as_str(),
            grace(request.grace_seconds),
        )
        .await;
    if let Some(response) = response {
        ctx.reply(inbound.reply, &response).await;
    }
}

pub(crate) async fn write_stdin(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<WriteStdinRequest>::decode(&msg, K8sVerb::WriteStdin) else {
        return;
    };
    let request = inbound.request;
    let response = ctx
        .provider
        .write_stdin_for_lease(
            request.lease_id(),
            request.execution_id().as_str(),
            request.seq(),
            request.data().clone(),
            request.eof(),
        )
        .await;
    if let Some(response) = response {
        ctx.reply(inbound.reply, &response).await;
    }
}
