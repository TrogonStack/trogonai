use super::Bridge;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, agent};
use crate::session_id::AcpSessionId;
use agent_client_protocol::ErrorCode;
use agent_client_protocol::{Error, PromptRequest, PromptResponse, Result};
use std::time::Duration;
use tracing::{info, instrument, warn};
use trogon_std::time::GetElapsed;

/// Threshold above which timeout messages use seconds instead of milliseconds.
const TIMEOUT_MSG_SECS_THRESHOLD: Duration = Duration::from_secs(60);

#[instrument(
    name = "acp.session.prompt",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N: RequestClient + PublishClient + FlushClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: PromptRequest,
) -> Result<PromptResponse> {
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Prompt request");

    let result = async {
        let session_id = AcpSessionId::try_from(&args.session_id).map_err(|e| {
            bridge.metrics.record_error("prompt", "invalid_session_id");
            Error::new(
                ErrorCode::InvalidParams.into(),
                format!("Invalid session ID: {}", e),
            )
        })?;

        let nats = bridge.nats();
        let subject = agent::session_prompt(bridge.config.acp_prefix(), session_id.as_str());

        let (rx, _waiter_guard) = bridge
            .pending_session_prompt_responses
            .register_waiter(args.session_id.clone())
            .map_err(|_| {
                bridge
                    .metrics
                    .record_error("prompt", "duplicate_prompt_waiter");
                Error::new(
                    ErrorCode::InvalidParams.into(),
                    format!("Duplicate prompt request for session {}", args.session_id),
                )
            })?;

        let publish_options = nats::PublishOptions::builder()
            .flush_policy(nats::FlushPolicy::no_retries())
            .build();

        if let Err(e) = nats::publish(nats, &subject, &args, publish_options).await {
            bridge
                .metrics
                .record_error("prompt", "prompt_publish_failed");
            warn!(session_id = %args.session_id, error = %e, "Failed to publish prompt request");
            return Err(Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to publish prompt request: {}", e),
            ));
        }

        let timeout_result = tokio::time::timeout(bridge.config.prompt_timeout(), rx).await;

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
                        bridge
                            .metrics
                            .record_error("prompt", "prompt_response_parse_failed");
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
                    bridge
                        .metrics
                        .record_error("prompt", "prompt_channel_closed");
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
                    .mark_prompt_waiter_timed_out(args.session_id.clone(), &bridge.clock);

                let timeout = bridge.config.prompt_timeout();
                let timeout_msg = if timeout >= TIMEOUT_MSG_SECS_THRESHOLD {
                    format!(
                        "Prompt request timed out after {:.3}s",
                        timeout.as_secs_f64()
                    )
                } else {
                    format!("Prompt request timed out after {}ms", timeout.as_millis())
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
    }
    .await;
    bridge.metrics.record_request(
        "prompt",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use agent_client_protocol::{
        Agent, ErrorCode, PromptRequest, PromptResponse, SessionId, StopReason,
    };
    use opentelemetry::Value;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{
        PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter,
    };
    use std::time::Duration;
    use trogon_nats::AdvancedMockNatsClient;

    fn mock_bridge() -> (
        AdvancedMockNatsClient,
        Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock>,
    ) {
        let mock = AdvancedMockNatsClient::new();
        let config = Config::for_test("acp").with_prompt_timeout(Duration::from_millis(100));
        let bridge = Bridge::new(
            mock.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            config,
        );
        (mock, bridge)
    }

    fn mock_bridge_with_metrics() -> (
        AdvancedMockNatsClient,
        Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock>,
        InMemoryMetricExporter,
        SdkMeterProvider,
    ) {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("acp-nats-test");
        let config = Config::for_test("acp").with_prompt_timeout(Duration::from_millis(100));

        let mock = AdvancedMockNatsClient::new();
        let bridge = Bridge::new(mock.clone(), trogon_std::time::SystemClock, &meter, config);
        (mock, bridge, exporter, provider)
    }

    fn has_request_metric(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
        method: &str,
        expected_success: bool,
    ) -> bool {
        finished_metrics
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .flat_map(|sm| sm.metrics())
            .find(|m| m.name() == "acp.request.count")
            .and_then(|metric| {
                let data = metric.data();
                if let AggregatedMetrics::U64(MetricData::Sum(s)) = data {
                    s.data_points()
                        .find(|dp| {
                            let mut method_ok = false;
                            let mut success_ok = false;
                            for attr in dp.attributes() {
                                if attr.key.as_str() == "method" {
                                    method_ok = attr.value.as_str() == method;
                                } else if attr.key.as_str() == "success" {
                                    success_ok = attr.value == Value::from(expected_success);
                                }
                            }
                            method_ok && success_ok
                        })
                        .map(|_| ())
                } else {
                    None
                }
            })
            .is_some()
    }

    fn has_error_metric(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
        operation: &str,
        reason: &str,
    ) -> bool {
        finished_metrics
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .flat_map(|sm| sm.metrics())
            .find(|m| m.name() == "acp.errors.total")
            .and_then(|metric| {
                let data = metric.data();
                if let AggregatedMetrics::U64(MetricData::Sum(s)) = data {
                    s.data_points()
                        .find(|dp| {
                            let mut operation_ok = false;
                            let mut reason_ok = false;
                            for attr in dp.attributes() {
                                if attr.key.as_str() == "operation" {
                                    operation_ok = attr.value.as_str() == operation;
                                } else if attr.key.as_str() == "reason" {
                                    reason_ok = attr.value.as_str() == reason;
                                }
                            }
                            operation_ok && reason_ok
                        })
                        .map(|_| ())
                } else {
                    None
                }
            })
            .is_some()
    }

    #[tokio::test]
    async fn prompt_validates_session_id() {
        let (_mock, bridge) = mock_bridge();
        let err = bridge
            .prompt(PromptRequest::new("invalid.session", vec![]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Invalid session ID"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn prompt_rejects_duplicate_waiter_for_same_session() {
        let (_mock, bridge) = mock_bridge();
        let (_rx, _guard) = bridge
            .pending_session_prompt_responses
            .register_waiter(agent_client_protocol::SessionId::from("s1"))
            .unwrap();

        let err = bridge
            .prompt(PromptRequest::new("s1", vec![]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Duplicate prompt request"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn prompt_waiter_guard_cleans_up_on_cancellation() {
        let (_mock, bridge) = mock_bridge();
        let bridge = std::sync::Arc::new(bridge);
        let bridge_spawn = std::sync::Arc::clone(&bridge);
        let bridge_after = std::sync::Arc::clone(&bridge);
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let handle = local.spawn_local(async move {
                    bridge_spawn.prompt(PromptRequest::new("s1", vec![])).await
                });
                tokio::time::sleep(Duration::from_millis(5)).await;
                handle.abort();
                let _ = handle.await;
                let (rx, _guard) = bridge_after
                    .pending_session_prompt_responses
                    .register_waiter(SessionId::from("s1"))
                    .expect("waiter should be free after cancelled prompt dropped guard");
                bridge_after
                    .pending_session_prompt_responses
                    .resolve_waiter(
                        &SessionId::from("s1"),
                        Ok(PromptResponse::new(StopReason::EndTurn)),
                    );
                let result = rx.await.unwrap().unwrap();
                assert_eq!(result.stop_reason, StopReason::EndTurn);
            })
            .await;
    }

    #[tokio::test]
    async fn prompt_returns_error_when_response_times_out() {
        let (_mock, bridge) = mock_bridge();

        let err = bridge
            .prompt(PromptRequest::new("s1", vec![]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("timed out"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn prompt_publishes_to_correct_subject() {
        let (mock, bridge) = mock_bridge();

        let _ = bridge.prompt(PromptRequest::new("s1", vec![])).await;

        let published = mock.published_messages();
        assert!(
            published.contains(&"acp.s1.agent.session.prompt".to_string()),
            "expected publish to acp.s1.agent.session.prompt, got: {:?}",
            published
        );
    }

    #[tokio::test]
    async fn prompt_resolves_waiter_with_response() {
        let (_mock, bridge) = mock_bridge();
        let (rx, _guard) = bridge
            .pending_session_prompt_responses
            .register_waiter(agent_client_protocol::SessionId::from("s1"))
            .unwrap();

        bridge.pending_session_prompt_responses.resolve_waiter(
            &agent_client_protocol::SessionId::from("s1"),
            Ok(PromptResponse::new(StopReason::EndTurn)),
        );

        let result = rx.await.unwrap().unwrap();
        assert_eq!(result.stop_reason, StopReason::EndTurn);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn prompt_records_metrics_on_success() {
        let (_mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        let bridge = std::sync::Arc::new(bridge);
        let bridge_prompt = std::sync::Arc::clone(&bridge);
        let bridge_resolve = std::sync::Arc::clone(&bridge);
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let prompt_handle = local.spawn_local(async move {
                    bridge_prompt.prompt(PromptRequest::new("s1", vec![])).await
                });
                tokio::time::sleep(Duration::from_millis(5)).await;
                bridge_resolve
                    .pending_session_prompt_responses
                    .resolve_waiter(
                        &SessionId::from("s1"),
                        Ok(PromptResponse::new(StopReason::EndTurn)),
                    );
                let result = prompt_handle.await.unwrap();
                assert!(result.is_ok());
                assert_eq!(result.unwrap().stop_reason, StopReason::EndTurn);
            })
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "prompt", true),
            "expected acp.request.count with method=prompt, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn prompt_records_error_metric_on_invalid_session_id() {
        let (_mock, bridge, exporter, provider) = mock_bridge_with_metrics();

        let _ = bridge
            .prompt(PromptRequest::new("invalid.session", vec![]))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "prompt", false),
            "expected acp.request.count with method=prompt, success=false"
        );
        assert!(
            has_error_metric(&finished_metrics, "prompt", "invalid_session_id"),
            "expected acp.errors.total with operation=prompt, reason=invalid_session_id"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn prompt_records_error_metric_on_publish_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_publish_count(1);

        let _ = bridge.prompt(PromptRequest::new("s1", vec![])).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_error_metric(&finished_metrics, "prompt", "prompt_publish_failed"),
            "expected acp.errors.total with operation=prompt, reason=prompt_publish_failed"
        );
        assert!(
            has_request_metric(&finished_metrics, "prompt", false),
            "request metric records publish outcome; success=false when publish fails"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn prompt_returns_error_when_response_parse_fails() {
        let (_mock, bridge) = mock_bridge();
        let bridge = std::sync::Arc::new(bridge);
        let bridge_prompt = std::sync::Arc::clone(&bridge);
        let bridge_resolve = std::sync::Arc::clone(&bridge);
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let prompt_handle = local.spawn_local(async move {
                    bridge_prompt.prompt(PromptRequest::new("s1", vec![])).await
                });
                tokio::time::sleep(Duration::from_millis(5)).await;
                bridge_resolve
                    .pending_session_prompt_responses
                    .resolve_waiter(&SessionId::from("s1"), Err("parse error".to_string()));
                let result = prompt_handle.await.unwrap();
                let err = result.unwrap_err();
                assert!(err.to_string().contains("parse failed"));
                assert_eq!(err.code, ErrorCode::InternalError);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn prompt_returns_error_when_channel_closed() {
        let (_mock, bridge) = mock_bridge();
        let bridge = std::sync::Arc::new(bridge);
        let bridge_prompt = std::sync::Arc::clone(&bridge);
        let bridge_remove = std::sync::Arc::clone(&bridge);
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let prompt_handle = local.spawn_local(async move {
                    bridge_prompt.prompt(PromptRequest::new("s1", vec![])).await
                });
                tokio::time::sleep(Duration::from_millis(5)).await;
                bridge_remove
                    .pending_session_prompt_responses
                    .remove_waiter(&SessionId::from("s1"));
                let result = prompt_handle.await.unwrap();
                let err = result.unwrap_err();
                assert!(err.to_string().contains("channel closed"));
                assert_eq!(err.code, ErrorCode::InternalError);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn prompt_success_logs_completion() {
        let (_mock, bridge) = mock_bridge();
        let bridge = std::sync::Arc::new(bridge);
        let bridge_prompt = std::sync::Arc::clone(&bridge);
        let bridge_resolve = std::sync::Arc::clone(&bridge);
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let prompt_handle = local.spawn_local(async move {
                    bridge_prompt.prompt(PromptRequest::new("s1", vec![])).await
                });
                tokio::time::sleep(Duration::from_millis(5)).await;
                bridge_resolve
                    .pending_session_prompt_responses
                    .resolve_waiter(
                        &SessionId::from("s1"),
                        Ok(PromptResponse::new(StopReason::EndTurn)),
                    );
                let result = prompt_handle.await.unwrap();
                assert!(result.is_ok());
                assert_eq!(result.unwrap().stop_reason, StopReason::EndTurn);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn prompt_timeout_message_uses_milliseconds_when_under_60s() {
        let (_mock, bridge) = mock_bridge();
        let err = bridge
            .prompt(PromptRequest::new("s1", vec![]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("ms"));
        assert!(!err.to_string().contains("s."));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn prompt_timeout_message_uses_seconds_when_60s_or_more() {
        let (mock, _) = mock_bridge();
        let config = Config::for_test("acp").with_prompt_timeout(Duration::from_secs(60));
        let bridge = Bridge::new(
            mock.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            config,
        );
        let bridge = std::sync::Arc::new(bridge);
        let bridge_prompt = std::sync::Arc::clone(&bridge);
        let local = tokio::task::LocalSet::new();
        let err = local
            .run_until(async {
                let handle = local.spawn_local(async move {
                    bridge_prompt.prompt(PromptRequest::new("s1", vec![])).await
                });
                tokio::time::advance(Duration::from_secs(61)).await;
                handle.await.unwrap().unwrap_err()
            })
            .await;
        assert!(err.to_string().contains("s"));
        assert!(err.to_string().contains("60"));
    }
}
