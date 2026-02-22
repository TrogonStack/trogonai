use super::Bridge;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, agent};
use agent_client_protocol::ErrorCode;
use agent_client_protocol::{Error, PromptRequest, PromptResponse, Result, StopReason};
use trogon_std::time::GetElapsed;
use tracing::{info, instrument, warn};

#[instrument(
    name = "acp.session.prompt",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<
    N: RequestClient + PublishClient + FlushClient,
    C: GetElapsed,
>(
    bridge: &Bridge<N, C>,
    args: PromptRequest,
) -> Result<PromptResponse> {
    struct PromptSlotGuard<'a, N, C>(&'a Bridge<N, C>)
    where
        N: RequestClient + PublishClient + FlushClient,
        C: GetElapsed;

    impl<'a, N, C> Drop for PromptSlotGuard<'a, N, C>
    where
        N: RequestClient + PublishClient + FlushClient,
        C: GetElapsed,
    {
        fn drop(&mut self) {
            self.0.release_prompt_slot();
        }
    }

    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Prompt request");

    let result = if !bridge.try_acquire_prompt_slot() {
        bridge.metrics.record_error("prompt", "agent_backpressure_rejected");
        Err(Error::new(
            ErrorCode::InternalError.into(),
            "Bridge overloaded - too many concurrent prompt requests",
        ))
    } else {
        let _slot_guard = PromptSlotGuard(bridge);
        (async {
            let session_id = bridge.validate_session(&args.session_id)?;

            // Use atomic take_if_cancelled to check and clear in one operation
            if bridge
                .cancelled_sessions
                .take_if_cancelled(&args.session_id, &bridge.clock)
                .is_some()
            {
                info!(session_id = %args.session_id, "Prompt cancelled before start");
                return Ok(PromptResponse::new(StopReason::Cancelled));
            }

            let nats = bridge.nats();
            let subject = agent::session_prompt(&bridge.config.acp_prefix, session_id.as_str());

            let rx = bridge
                .pending_session_prompt_responses
                .register_waiter(args.session_id.clone())
                .map_err(|_| {
                    bridge.metrics.record_error("prompt", "duplicate_prompt_waiter");
                    Error::new(
                        ErrorCode::InvalidParams.into(),
                        format!(
                            "Duplicate prompt request for session {}",
                            args.session_id
                        ),
                    )
                })?;

            // Re-check after registration to close the race window where a cancel
            // arrives between the initial check and waiter registration.
            if bridge
                .cancelled_sessions
                .take_if_cancelled(&args.session_id, &bridge.clock)
                .is_some()
            {
                bridge
                    .pending_session_prompt_responses
                    .remove_waiter(&args.session_id);
                info!(session_id = %args.session_id, "Prompt cancelled before publish");
                return Ok(PromptResponse::new(StopReason::Cancelled));
            }

            // CRITICAL: Use flush to ensure prompt publish leaves the process.
            // Without flush, NATS buffers locally and the message may never be sent.
            // This would cause a silent 2-hour hang waiting for a response that never comes.
            let publish_options = nats::PublishOptions::builder()
                .flush_policy(nats::FlushPolicy::no_retries())
                .build();

            if let Err(e) = nats::publish(nats, &subject, &args, publish_options).await {
                bridge
                    .pending_session_prompt_responses
                    .remove_waiter(&args.session_id);
                warn!(session_id = %args.session_id, error = %e, "Failed to publish prompt request");
                return Err(Error::new(
                    ErrorCode::InternalError.into(),
                    format!("Failed to publish prompt request: {}", e),
                ));
            }

            let timeout_result = tokio::time::timeout(bridge.config.prompt_timeout, rx).await;

            let result = match timeout_result {
                Ok(wait_result) => match wait_result {
                    Ok(response_result) => match response_result {
                        Ok(response) => Ok(response),
                        Err(err_msg) => {
                            warn!(
                                session_id = %args.session_id,
                                error = %err_msg,
                                "Prompt response payload parse error"
                            );
                            bridge.metrics.record_error("prompt", "prompt_response_parse_failed");
                            Err(Error::new(
                                ErrorCode::InternalError.into(),
                                format!("Prompt response parse failed: {}", err_msg),
                            ))
                        }
                    },
                    Err(_) => {
                        warn!(
                            session_id = %args.session_id,
                            "Prompt response channel closed unexpectedly"
                        );
                        bridge.metrics.record_error("prompt", "prompt_channel_closed");
                        Err(Error::new(
                            ErrorCode::InternalError.into(),
                            "Prompt response channel closed unexpectedly",
                        ))
                    }
                },
                Err(_) => {
                    warn!(session_id = %args.session_id, "Prompt request timed out");
                    bridge.metrics.record_error("prompt", "prompt_timeout");
                    bridge
                        .pending_session_prompt_responses
                        .remove_waiter(&args.session_id);
                    bridge
                        .pending_session_prompt_responses
                        .mark_prompt_waiter_timed_out(args.session_id.clone(), &bridge.clock);

                    let timeout_ms = bridge.config.prompt_timeout.as_millis();
                    let timeout_msg = if timeout_ms >= 60000 {
                        format!(
                            "Prompt request timed out after {:.3}s",
                            bridge.config.prompt_timeout.as_secs_f64()
                        )
                    } else {
                        format!("Prompt request timed out after {timeout_ms}ms")
                    };

                    Err(Error::new(ErrorCode::InternalError.into(), timeout_msg))
                }
            };

            if let Ok(ref response) = result {
                info!(
                    session_id = %args.session_id,
                    stop_reason = ?response.stop_reason,
                    "Prompt completed"
                );
            }

            result
        })
        .await
    };
    bridge.metrics.record_request(
        "prompt",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}
