use std::sync::Arc;

use trogon_sandbox_contract::{PollCommandRequest, PollCommandResponse, RunCommandRequest, RunCommandResponse};

use crate::run_command_reply_fit::fit_run_command_response;
use crate::serve::route_context::RouteContext;

/// A poll reply trimmed to fit one bus message, the same way a run reply is.
pub(crate) fn fit_poll_command_response(response: PollCommandResponse, max_payload: usize) -> PollCommandResponse {
    let PollCommandResponse::Completed(output) = response else {
        return response;
    };
    match fit_run_command_response(RunCommandResponse::Completed(output), max_payload) {
        RunCommandResponse::Completed(fitted) => PollCommandResponse::Completed(fitted),
        RunCommandResponse::Denied(error) => PollCommandResponse::Denied(error),
        RunCommandResponse::Started(_) => PollCommandResponse::Running,
    }
}

pub(crate) async fn run(context: Arc<RouteContext>, request: RunCommandRequest) -> RunCommandResponse {
    let response = context.broker.handle_run_command(request).await;
    fit_run_command_response(response, context.nats.server_info().max_payload)
}

pub(crate) async fn poll(context: Arc<RouteContext>, request: PollCommandRequest) -> PollCommandResponse {
    let response = context.broker.handle_poll_command(request).await;
    fit_poll_command_response(response, context.nats.server_info().max_payload)
}

#[cfg(test)]
mod tests;
