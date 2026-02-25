use super::Bridge;
use crate::error::AGENT_UNAVAILABLE;
use crate::nats::{
    self, ExtSessionReady, FlushClient, FlushPolicy, PublishClient, PublishOptions, RequestClient,
    RetryPolicy, agent,
};
use crate::telemetry::metrics::Metrics;
use agent_client_protocol::{
    Error, ErrorCode, NewSessionRequest, NewSessionResponse, Result, SessionId,
};
use std::time::Duration;
use tracing::{Span, info, instrument, warn};
use trogon_nats::NatsError;
use trogon_std::time::GetElapsed;

/// Delay before publishing `session.ready` to NATS.
///
/// The `Agent` trait returns the response value *before* the transport layer
/// serializes and writes it to the client. Without a delay the spawned task
/// could publish `session.ready` to NATS before the client has received the
/// `session/new` response, violating the ordering guarantee documented on
/// [`ExtSessionReady`].
///
/// A post-send callback from the transport would be the ideal fix, but the
/// external `agent_client_protocol` crate does not expose one. This constant
/// delay provides a practical safety margin (serialization + write is typically
/// sub-millisecond).
const SESSION_READY_DELAY: Duration = Duration::from_millis(100);

fn map_new_session_error(e: NatsError) -> Error {
    match &e {
        NatsError::Timeout { subject } => {
            warn!(subject = %subject, "new_session request timed out");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                "New session request timed out; agent may be overloaded or unavailable",
            )
        }
        NatsError::Request { subject, error } => {
            warn!(subject = %subject, error = %error, "new_session NATS request failed");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                format!("Agent unavailable: {}", error),
            )
        }
        NatsError::Serialize(inner) => {
            warn!(error = %inner, "failed to serialize new_session request");
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to serialize new_session request: {}", inner),
            )
        }
        NatsError::Deserialize(inner) => {
            warn!(error = %inner, "failed to deserialize new_session response");
            Error::new(
                ErrorCode::InternalError.into(),
                "Invalid response from agent",
            )
        }
        _ => {
            warn!(error = %e, "new_session NATS request failed");
            Error::new(
                ErrorCode::InternalError.into(),
                "New session request failed",
            )
        }
    }
}

#[instrument(
    name = "acp.session.new",
    skip(bridge, args),
    fields(cwd = ?args.cwd, mcp_servers = args.mcp_servers.len(), session_id = tracing::field::Empty)
)]
pub async fn handle<N: RequestClient + PublishClient + FlushClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: NewSessionRequest,
) -> Result<NewSessionResponse> {
    let start = bridge.clock.now();

    info!(cwd = ?args.cwd, mcp_servers = args.mcp_servers.len(), "New session request");

    let nats = bridge.nats();
    let subject = agent::session_new(bridge.config.acp_prefix());

    let result = nats::request_with_timeout::<N, NewSessionRequest, NewSessionResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_new_session_error);

    if let Ok(ref response) = result {
        Span::current().record("session_id", response.session_id.to_string().as_str());
        info!(session_id = %response.session_id, "Session created");

        let nats = bridge.nats.clone();
        let prefix = bridge.config.acp_prefix.clone();
        let session_id = response.session_id.clone();
        let metrics = bridge.metrics.clone();
        // TODO: track the JoinHandle so we can drain in-flight publishes on graceful shutdown.
        tokio::spawn(async move {
            publish_session_ready(&nats, prefix.as_str(), &session_id, &metrics).await;
        });
    }

    bridge.metrics.record_request(
        "new_session",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

async fn publish_session_ready<N: PublishClient + FlushClient>(
    nats: &N,
    prefix: &str,
    session_id: &SessionId,
    metrics: &Metrics,
) {
    tokio::time::sleep(SESSION_READY_DELAY).await;

    let subject = agent::ext_session_ready(prefix, &session_id.to_string());
    info!(session_id = %session_id, subject = %subject, "Publishing session.ready");

    let message = ExtSessionReady::new(session_id.clone());

    let options = PublishOptions::builder()
        .publish_retry_policy(RetryPolicy::standard())
        .flush_policy(FlushPolicy::standard())
        .build();

    if let Err(e) = nats::publish(nats, &subject, &message, options).await {
        warn!(
            error = %e,
            session_id = %session_id,
            "Failed to publish session.ready"
        );
        metrics.record_error("session_ready", "session_ready_publish_failed");
    } else {
        info!(session_id = %session_id, "Published session.ready");
    }
}

#[cfg(test)]
mod tests {
    use super::{Bridge, map_new_session_error};
    use crate::config::Config;
    use crate::error::AGENT_UNAVAILABLE;
    use agent_client_protocol::{
        Agent, ErrorCode, NewSessionRequest, NewSessionResponse, SessionId,
    };
    use opentelemetry::Value;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{
        PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter,
    };
    use std::time::Duration;
    use trogon_nats::{AdvancedMockNatsClient, NatsError};

    fn assert_new_session_metric_recorded(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
        expected_success: bool,
    ) {
        let found = finished_metrics
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .any(|sm| {
                sm.metrics().any(|metric| {
                    if metric.name() != "acp.request.count" {
                        return false;
                    }
                    let data = metric.data();
                    let sum = match data {
                        AggregatedMetrics::U64(MetricData::Sum(s)) => s,
                        _ => return false,
                    };
                    sum.data_points().any(|dp| {
                        let mut method_ok = false;
                        let mut success_ok = false;
                        for attr in dp.attributes() {
                            if attr.key.as_str() == "method" {
                                method_ok = attr.value.as_str() == "new_session";
                            } else if attr.key.as_str() == "success" {
                                success_ok = attr.value == Value::from(expected_success);
                            }
                        }
                        method_ok && success_ok
                    })
                })
            });
        assert!(
            found,
            "expected acp.request.count datapoint with method=new_session, success={}",
            expected_success
        );
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

        let mock = AdvancedMockNatsClient::new();
        let bridge = Bridge::new(
            mock.clone(),
            trogon_std::time::SystemClock,
            &meter,
            Config::for_test("acp"),
        );
        (mock, bridge, exporter, provider)
    }

    fn mock_bridge() -> (
        AdvancedMockNatsClient,
        Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock>,
    ) {
        let mock = AdvancedMockNatsClient::new();
        let bridge = Bridge::new(
            mock.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            Config::for_test("acp"),
        );
        (mock, bridge)
    }

    fn set_json_response<T: serde::Serialize>(
        mock: &AdvancedMockNatsClient,
        subject: &str,
        resp: &T,
    ) {
        let bytes = serde_json::to_vec(resp).unwrap();
        mock.set_response(subject, bytes.into());
    }

    #[tokio::test]
    async fn new_session_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let session_id = SessionId::from("test-session-1");
        let expected = NewSessionResponse::new(session_id.clone());
        set_json_response(&mock, "acp.agent.session.new", &expected);

        let request = NewSessionRequest::new(".");
        let result = bridge.new_session(request).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.session_id, session_id);
    }

    #[tokio::test]
    async fn new_session_returns_error_when_nats_request_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = NewSessionRequest::new(".");
        let err = bridge.new_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn new_session_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.agent.session.new", "not json".into());

        let request = NewSessionRequest::new(".");
        let err = bridge.new_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn new_session_records_metrics_on_success() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        let session_id = SessionId::from("test-session-1");
        set_json_response(
            &mock,
            "acp.agent.session.new",
            &NewSessionResponse::new(session_id),
        );

        let _ = bridge.new_session(NewSessionRequest::new(".")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert_new_session_metric_recorded(&finished_metrics, true);
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn new_session_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge.new_session(NewSessionRequest::new(".")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert_new_session_metric_recorded(&finished_metrics, false);
        provider.shutdown().unwrap();
    }

    #[test]
    fn map_new_session_error_timeout() {
        let err = map_new_session_error(NatsError::Timeout {
            subject: "acp.agent.session.new".into(),
        });
        assert!(err.to_string().contains("timed out"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn map_new_session_error_request() {
        let err = map_new_session_error(NatsError::Request {
            subject: "acp.agent.session.new".into(),
            error: "connection refused".into(),
        });
        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn map_new_session_error_serialize() {
        let serde_err = serde_json::to_vec(&FailsSerialize).unwrap_err();
        let err = map_new_session_error(NatsError::Serialize(serde_err));
        assert!(err.to_string().contains("serialize"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_new_session_error_deserialize() {
        let serde_err = serde_json::from_str::<NewSessionResponse>("{}").unwrap_err();
        let err = map_new_session_error(NatsError::Deserialize(serde_err));
        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_new_session_error_other() {
        let err = map_new_session_error(NatsError::Other("misc failure".into()));
        assert!(err.to_string().contains("New session request failed"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    struct FailsSerialize;
    impl serde::Serialize for FailsSerialize {
        fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("test serialize failure"))
        }
    }
}
