use super::Bridge;
use crate::config::AcpPrefix;
use crate::error::AGENT_UNAVAILABLE;
use crate::nats::{
    self, ExtSessionReady, FlushClient, FlushPolicy, PublishClient, PublishOptions, RequestClient,
    RetryPolicy, agent,
};
use crate::session_id::AcpSessionId;
use crate::telemetry::metrics::Metrics;
use agent_client_protocol::{Error, ErrorCode, LoadSessionRequest, LoadSessionResponse, Result};
use std::time::Duration;
use tracing::{info, instrument, warn};
use trogon_nats::NatsError;
use trogon_std::time::GetElapsed;

const SESSION_READY_DELAY: Duration = Duration::from_millis(100);

fn map_load_session_error(e: NatsError) -> Error {
    match &e {
        NatsError::Timeout { subject } => {
            warn!(subject = %subject, "load_session request timed out");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                "Load session request timed out; agent may be overloaded or unavailable",
            )
        }
        NatsError::Request { subject, error } => {
            warn!(subject = %subject, error = %error, "load_session NATS request failed");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                format!("Agent unavailable: {}", error),
            )
        }
        NatsError::Serialize(inner) => {
            warn!(error = %inner, "failed to serialize load_session request");
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to serialize load_session request: {}", inner),
            )
        }
        NatsError::Deserialize(inner) => {
            warn!(error = %inner, "failed to deserialize load_session response");
            Error::new(
                ErrorCode::InternalError.into(),
                "Invalid response from agent",
            )
        }
        _ => {
            warn!(error = %e, "load_session NATS request failed");
            Error::new(
                ErrorCode::InternalError.into(),
                "Load session request failed",
            )
        }
    }
}

#[instrument(
    name = "acp.session.load",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N: RequestClient + PublishClient + FlushClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: LoadSessionRequest,
) -> Result<LoadSessionResponse> {
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Load session request");

    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|e| {
        bridge
            .metrics
            .record_error("session_validate", "invalid_session_id");
        Error::new(
            ErrorCode::InvalidParams.into(),
            format!("Invalid session ID: {}", e),
        )
    })?;
    let nats = bridge.nats();
    let subject = agent::session_load(bridge.config.acp_prefix(), session_id.as_str());

    let result = nats::request_with_timeout::<N, LoadSessionRequest, LoadSessionResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_load_session_error);

    if result.is_ok() {
        let nats = bridge.nats.clone();
        let prefix = bridge.config.acp_prefix.clone();
        let session_id = session_id.clone();
        let metrics = bridge.metrics.clone();
        // TODO: track the JoinHandle so we can drain in-flight publishes on graceful shutdown.
        tokio::spawn(async move {
            publish_session_ready(&nats, &prefix, &session_id, &metrics).await;
        });
    }

    bridge.metrics.record_request(
        "load_session",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

async fn publish_session_ready<N: PublishClient + FlushClient>(
    nats: &N,
    prefix: &AcpPrefix,
    session_id: &AcpSessionId,
    metrics: &Metrics,
) {
    tokio::time::sleep(SESSION_READY_DELAY).await;

    let subject = agent::ext_session_ready(prefix.as_str(), session_id.as_str());
    info!(session_id = %session_id, subject = %subject, "Publishing session.ready");

    let message = ExtSessionReady::new(agent_client_protocol::SessionId::from(
        session_id.to_string(),
    ));

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
    use super::{Bridge, map_load_session_error};
    use crate::config::Config;
    use crate::error::AGENT_UNAVAILABLE;
    use agent_client_protocol::{Agent, ErrorCode, LoadSessionRequest, LoadSessionResponse};
    use opentelemetry::Value;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{
        PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter,
    };
    use std::time::Duration;
    use trogon_nats::{AdvancedMockNatsClient, NatsError};

    fn has_session_ready_error_metric(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
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
                                    operation_ok = attr.value.as_str() == "session_ready";
                                } else if attr.key.as_str() == "reason" {
                                    reason_ok =
                                        attr.value.as_str() == "session_ready_publish_failed";
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

    fn assert_session_ready_error_recorded(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
    ) {
        assert!(
            has_session_ready_error_metric(finished_metrics),
            "expected acp.errors.total datapoint with operation=session_ready, reason=session_ready_publish_failed"
        );
    }

    fn has_load_session_metric(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
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
                                    method_ok = attr.value.as_str() == "load_session";
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

    fn assert_load_session_metric_recorded(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
        expected_success: bool,
    ) {
        assert!(
            has_load_session_metric(finished_metrics, expected_success),
            "expected acp.request.count datapoint with method=load_session, success={}",
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

    struct FailsSerialize;
    impl serde::Serialize for FailsSerialize {
        fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("test serialize failure"))
        }
    }

    #[tokio::test]
    async fn load_session_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = LoadSessionResponse::new();
        set_json_response(&mock, "acp.s1.agent.session.load", &expected);

        let request = LoadSessionRequest::new("s1", ".");
        let result = bridge.load_session(request).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn load_session_returns_error_when_nats_request_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = LoadSessionRequest::new("s1", ".");
        let err = bridge.load_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn load_session_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.s1.agent.session.load", "not json".into());

        let request = LoadSessionRequest::new("s1", ".");
        let err = bridge.load_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn load_session_records_metrics_on_success() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.s1.agent.session.load",
            &LoadSessionResponse::new(),
        );

        let _ = bridge
            .load_session(LoadSessionRequest::new("s1", "."))
            .await;

        tokio::time::sleep(Duration::from_millis(150)).await;
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert_load_session_metric_recorded(&finished_metrics, true);
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn load_session_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge
            .load_session(LoadSessionRequest::new("s1", "."))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert_load_session_metric_recorded(&finished_metrics, false);
        provider.shutdown().unwrap();
    }

    #[test]
    fn map_load_session_error_timeout() {
        let err = map_load_session_error(NatsError::Timeout {
            subject: "acp.s1.agent.session.load".into(),
        });
        assert!(err.to_string().contains("timed out"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn map_load_session_error_request() {
        let err = map_load_session_error(NatsError::Request {
            subject: "acp.s1.agent.session.load".into(),
            error: "connection refused".into(),
        });
        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn map_load_session_error_serialize() {
        let serde_err = serde_json::to_vec(&FailsSerialize).unwrap_err();
        let err = map_load_session_error(NatsError::Serialize(serde_err));
        assert!(err.to_string().contains("serialize"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_load_session_error_deserialize() {
        let serde_err = serde_json::from_str::<LoadSessionResponse>("[]").unwrap_err();
        let err = map_load_session_error(NatsError::Deserialize(serde_err));
        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_load_session_error_other() {
        let err = map_load_session_error(NatsError::Other("misc failure".into()));
        assert!(err.to_string().contains("Load session request failed"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn has_session_ready_error_metric_returns_false_when_metric_is_histogram() {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("test");
        let histogram = meter
            .f64_histogram("acp.errors.total")
            .with_description("test")
            .build();
        histogram.record(1.0, &[]);
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!has_session_ready_error_metric(&finished_metrics));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_load_session_metric_returns_false_when_metric_is_histogram() {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("test");
        let histogram = meter
            .f64_histogram("acp.request.count")
            .with_description("test")
            .build();
        histogram.record(1.0, &[]);
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!has_load_session_metric(&finished_metrics, true));
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn load_session_validates_session_id() {
        let (_mock, bridge) = mock_bridge();
        let request = LoadSessionRequest::new("invalid.session.id", ".");
        let err = bridge.load_session(request).await.unwrap_err();
        assert!(err.to_string().contains("Invalid session ID"));
    }

    #[tokio::test]
    async fn load_session_records_error_when_session_ready_publish_fails() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.s1.agent.session.load",
            &LoadSessionResponse::new(),
        );
        mock.fail_publish_count(4);

        let _ = bridge
            .load_session(LoadSessionRequest::new("s1", "."))
            .await;

        tokio::time::sleep(Duration::from_millis(600)).await;
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert_session_ready_error_recorded(&finished_metrics);
        assert_load_session_metric_recorded(&finished_metrics, true);
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn load_session_publishes_session_ready_to_correct_subject() {
        let (mock, bridge) = mock_bridge();
        set_json_response(
            &mock,
            "acp.s1.agent.session.load",
            &LoadSessionResponse::new(),
        );

        let _ = bridge
            .load_session(LoadSessionRequest::new("s1", "."))
            .await;

        tokio::time::sleep(Duration::from_millis(300)).await;
        let published = mock.published_messages();
        assert!(
            published.contains(&"acp.s1.agent.ext.session.ready".to_string()),
            "expected publish to acp.s1.agent.ext.session.ready, got: {:?}",
            published
        );
    }
}
