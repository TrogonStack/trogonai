use super::Bridge;
use crate::AgentHandler;
use crate::agent::test_support::{
    MockJs, has_request_metric, mock_bridge, mock_bridge_with_metrics, set_json_response,
};
use crate::config::Config;
use crate::error::AGENT_UNAVAILABLE;
use agent_client_protocol::ErrorCode;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{Implementation, InitializeRequest, InitializeResponse};
use trogon_nats::AdvancedMockNatsClient;

#[tokio::test]
async fn initialize_forwards_request_and_returns_response() {
    let (mock, _js, bridge) = mock_bridge();
    let expected = InitializeResponse::new(ProtocolVersion::LATEST);
    set_json_response(&mock, "acp.v1.global.agent.initialize", &expected);

    let request = InitializeRequest::new(ProtocolVersion::LATEST);
    let result = bridge.initialize(request).await;

    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response.protocol_version, ProtocolVersion::LATEST);
}

#[tokio::test]
async fn initialize_logs_client_name_when_client_info_provided() {
    let (mock, _js, bridge) = mock_bridge();
    let expected = InitializeResponse::new(ProtocolVersion::LATEST);
    set_json_response(&mock, "acp.v1.global.agent.initialize", &expected);

    let request =
        InitializeRequest::new(ProtocolVersion::LATEST).client_info(Implementation::new("my-client", "1.0.0"));
    let result = bridge.initialize(request).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn initialize_returns_error_when_nats_request_fails() {
    let (mock, _js, bridge) = mock_bridge();
    mock.fail_next_request();

    let request = InitializeRequest::new(ProtocolVersion::LATEST);
    let err = bridge.initialize(request).await.unwrap_err();

    assert_eq!(err.code, ErrorCode::new(AGENT_UNAVAILABLE));
}

#[tokio::test]
async fn initialize_returns_error_when_response_is_invalid_json() {
    let (mock, _js, bridge) = mock_bridge();
    mock.set_response("acp.v1.global.agent.initialize", "not json".into());

    let request = InitializeRequest::new(ProtocolVersion::LATEST);
    let err = bridge.initialize(request).await.unwrap_err();

    assert_eq!(err.message, "Invalid response from agent");
    assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);
}

#[tokio::test]
async fn initialize_records_metrics_on_success() {
    let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
    set_json_response(
        &mock,
        "acp.v1.global.agent.initialize",
        &InitializeResponse::new(ProtocolVersion::LATEST),
    );

    let _ = bridge.initialize(InitializeRequest::new(ProtocolVersion::LATEST)).await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "initialize", true),
        "expected acp.requests with method=initialize, success=true"
    );
    provider.shutdown().unwrap();
}

#[tokio::test]
async fn initialize_records_metrics_on_failure() {
    let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
    mock.fail_next_request();

    let _ = bridge.initialize(InitializeRequest::new(ProtocolVersion::LATEST)).await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "initialize", false),
        "expected acp.requests with method=initialize, success=false"
    );
    provider.shutdown().unwrap();
}

#[tokio::test]
async fn handlers_use_custom_prefix() {
    let mock = AdvancedMockNatsClient::new();
    let bridge = Bridge::new(
        mock.clone(),
        MockJs::new(),
        trogon_std::time::SystemClock,
        &opentelemetry::global::meter("acp-nats-test"),
        Config::for_test("myorg.prod"),
    );
    let expected = InitializeResponse::new(ProtocolVersion::LATEST);
    set_json_response(&mock, "myorg.prod.v1.global.agent.initialize", &expected);

    let request = InitializeRequest::new(ProtocolVersion::LATEST);
    let result = bridge.initialize(request).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn the_initialize_and_every_later_request_name_the_client() {
    use crate::agent::test_support::set_js_response;
    use agent_client_protocol::v1::schema::{
        LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse,
    };

    let (mock, js, bridge) = mock_bridge();
    set_json_response(
        &mock,
        "acp.v1.global.agent.initialize",
        &InitializeResponse::new(ProtocolVersion::LATEST),
    );
    set_json_response(&mock, "acp.v1.global.agent.session.new", &NewSessionResponse::new("s1"));
    set_js_response(&js, &LoadSessionResponse::new());

    bridge
        .initialize(
            InitializeRequest::new(ProtocolVersion::LATEST).client_info(Implementation::new("discord", "1.0.0")),
        )
        .await
        .expect("initialize");
    bridge
        .new_session(NewSessionRequest::new("."))
        .await
        .expect("session/new");
    bridge
        .load_session(LoadSessionRequest::new("s1", "."))
        .await
        .expect("session/load");

    let requested = mock.requested_headers();
    assert_eq!(requested.len(), 2);
    for headers in &requested {
        assert_eq!(crate::requesting_client(Some(headers)), Some("discord"));
    }
    let commands = js.publisher.published_messages();
    assert_eq!(commands.len(), 1);
    assert_eq!(crate::requesting_client(Some(&commands[0].headers)), Some("discord"));
}

#[tokio::test]
async fn a_client_that_never_initializes_sends_no_name() {
    use agent_client_protocol::v1::schema::{NewSessionRequest, NewSessionResponse};

    let (mock, _js, bridge) = mock_bridge();
    set_json_response(&mock, "acp.v1.global.agent.session.new", &NewSessionResponse::new("s1"));

    bridge
        .new_session(NewSessionRequest::new("."))
        .await
        .expect("session/new");

    assert_eq!(crate::requesting_client(mock.requested_headers().first()), None);
}
