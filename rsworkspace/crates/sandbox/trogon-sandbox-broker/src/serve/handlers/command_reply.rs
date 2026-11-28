use std::sync::Arc;

use trogon_sandbox_contract::{PollCommandRequest, PollCommandResponse, RunCommandRequest, RunCommandResponse};
use trogon_sandbox_protocol::{fit_poll_command_response, fit_run_command_response};

use crate::serve::route_context::RouteContext;

pub(crate) async fn run(context: Arc<RouteContext>, request: RunCommandRequest) -> RunCommandResponse {
    let response = context.broker.handle_run_command(request).await;
    fit_run_command_response(response, context.nats.server_info().max_payload)
}

pub(crate) async fn poll(context: Arc<RouteContext>, request: PollCommandRequest) -> PollCommandResponse {
    let response = context.broker.handle_poll_command(request).await;
    fit_poll_command_response(response, context.nats.server_info().max_payload)
}
