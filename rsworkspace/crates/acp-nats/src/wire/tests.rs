use super::*;
use agent_client_protocol::Error;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, ClientCapabilities, ClientNesCapabilities, ConfigOptionUpdate, ContentBlock, ContentChunk,
    ElicitationCapabilities, ElicitationFormCapabilities, InitializeRequest, InitializeResponse, LoadSessionRequest,
    MessageId, NesCapabilities, NewSessionRequest, PlanEntry, PlanEntryPriority, PlanEntryStatus, PlanId, PlanRemoved,
    PlanUpdate, PlanUpdateContent, PositionEncodingKind, SessionConfigId, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigOptionValue, SessionInfoUpdate, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, UsageUpdate,
};
use std::path::PathBuf;

#[test]
fn decode_response_preserves_structured_error_data() {
    let data = serde_json::json!({ "retryable": true, "detail": "agent unavailable" });
    let error = Error::new(-32001, "boom").data(data.clone());
    let encoded = encode_agent_error(ResponseId::Number(7), &error).unwrap();

    let decoded: Result<serde_json::Value, Error> = decode_response(&encoded.headers, &encoded.body).unwrap();
    let recovered = decoded.unwrap_err();

    assert_eq!(recovered.message, "boom");
    assert_eq!(recovered.data, Some(data));
}

#[test]
fn new_session_request_additional_directories_survive_round_trip() {
    let request = NewSessionRequest::new("/workspace").additional_directories(vec![
        PathBuf::from("/workspace/extra"),
        PathBuf::from("/workspace/other"),
    ]);

    let encoded = encode_request("session/new", RequestId::Number(1), &request).unwrap();
    let decoded: NewSessionRequest = decode_request_params("session/new", &encoded.headers, &encoded.body).unwrap();

    assert_eq!(decoded.additional_directories, request.additional_directories);
}

#[test]
fn load_session_request_additional_directories_survive_round_trip() {
    let request =
        LoadSessionRequest::new("s1", "/workspace").additional_directories(vec![PathBuf::from("/workspace/extra")]);

    let encoded = encode_request("session/load", RequestId::Number(1), &request).unwrap();
    let decoded: LoadSessionRequest = decode_request_params("session/load", &encoded.headers, &encoded.body).unwrap();

    assert_eq!(decoded.additional_directories, request.additional_directories);
}

#[test]
fn initialize_request_elicitation_and_nes_capabilities_survive_round_trip() {
    let client_capabilities = ClientCapabilities::new()
        .elicitation(ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()))
        .nes(ClientNesCapabilities::new())
        .position_encodings(vec![PositionEncodingKind::Utf8]);
    let request = InitializeRequest::new(ProtocolVersion::LATEST).client_capabilities(client_capabilities);

    let encoded = encode_request("initialize", RequestId::Number(1), &request).unwrap();
    let decoded: InitializeRequest = decode_request_params("initialize", &encoded.headers, &encoded.body).unwrap();

    assert_eq!(
        decoded.client_capabilities.elicitation,
        request.client_capabilities.elicitation
    );
    assert_eq!(decoded.client_capabilities.nes, request.client_capabilities.nes);
    assert_eq!(
        decoded.client_capabilities.position_encodings,
        request.client_capabilities.position_encodings
    );
}

#[test]
fn initialize_response_nes_capabilities_survive_round_trip() {
    let response = InitializeResponse::new(ProtocolVersion::LATEST).agent_capabilities(
        AgentCapabilities::new()
            .nes(NesCapabilities::new())
            .position_encoding(PositionEncodingKind::Utf16),
    );

    let encoded = encode_success(ResponseId::Number(1), &response).unwrap();
    let decoded: Result<InitializeResponse, Error> = decode_response(&encoded.headers, &encoded.body).unwrap();
    let decoded = decoded.unwrap();

    assert_eq!(decoded.agent_capabilities.nes, response.agent_capabilities.nes);
    assert_eq!(
        decoded.agent_capabilities.position_encoding,
        response.agent_capabilities.position_encoding
    );
}

#[test]
fn session_update_plan_update_survives_round_trip() {
    let notification = SessionNotification::new(
        "s1",
        SessionUpdate::PlanUpdate(PlanUpdate::new(PlanUpdateContent::items(
            "plan-1",
            vec![PlanEntry::new(
                "write tests",
                PlanEntryPriority::High,
                PlanEntryStatus::InProgress,
            )],
        ))),
    );

    let encoded = encode_notification("session/update", &notification).unwrap();
    let decoded: SessionNotification =
        decode_notification_params("session/update", &encoded.headers, &encoded.body).unwrap();

    match decoded.update {
        SessionUpdate::PlanUpdate(update) => match update.plan {
            PlanUpdateContent::Items(items) => {
                assert_eq!(items.plan_id, PlanId::new("plan-1"));
                assert_eq!(items.entries.len(), 1);
                assert_eq!(items.entries[0].content, "write tests");
                assert_eq!(items.entries[0].priority, PlanEntryPriority::High);
                assert_eq!(items.entries[0].status, PlanEntryStatus::InProgress);
            }
            other => panic!("expected PlanUpdateContent::Items, got {other:?}"),
        },
        other => panic!("expected SessionUpdate::PlanUpdate, got {other:?}"),
    }
}

#[test]
fn session_update_plan_removed_survives_round_trip() {
    let notification = SessionNotification::new("s1", SessionUpdate::PlanRemoved(PlanRemoved::new("plan-1")));

    let encoded = encode_notification("session/update", &notification).unwrap();
    let decoded: SessionNotification =
        decode_notification_params("session/update", &encoded.headers, &encoded.body).unwrap();

    match decoded.update {
        SessionUpdate::PlanRemoved(removed) => assert_eq!(removed.plan_id, PlanId::new("plan-1")),
        other => panic!("expected SessionUpdate::PlanRemoved, got {other:?}"),
    }
}

#[test]
fn session_update_usage_update_survives_round_trip() {
    let notification = SessionNotification::new("s1", SessionUpdate::UsageUpdate(UsageUpdate::new(1234, 8192)));

    let encoded = encode_notification("session/update", &notification).unwrap();
    let decoded: SessionNotification =
        decode_notification_params("session/update", &encoded.headers, &encoded.body).unwrap();

    match decoded.update {
        SessionUpdate::UsageUpdate(usage) => {
            assert_eq!(usage.used, 1234);
            assert_eq!(usage.size, 8192);
        }
        other => panic!("expected SessionUpdate::UsageUpdate, got {other:?}"),
    }
}

#[test]
fn session_update_config_option_update_survives_round_trip() {
    let notification = SessionNotification::new(
        "s1",
        SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(vec![
            SessionConfigOption::boolean("verbose", "Verbose output", true),
            SessionConfigOption::boolean("temperature", "Temperature", true)
                .category(SessionConfigOptionCategory::ModelConfig),
        ])),
    );

    let encoded = encode_notification("session/update", &notification).unwrap();
    let decoded: SessionNotification =
        decode_notification_params("session/update", &encoded.headers, &encoded.body).unwrap();

    match decoded.update {
        SessionUpdate::ConfigOptionUpdate(update) => {
            assert_eq!(update.config_options.len(), 2);
            assert_eq!(update.config_options[0].id, SessionConfigId::new("verbose"));
            assert_eq!(update.config_options[0].name, "Verbose output");
            assert_eq!(update.config_options[0].category, None);
            assert_eq!(update.config_options[1].id, SessionConfigId::new("temperature"));
            assert_eq!(
                update.config_options[1].category,
                Some(SessionConfigOptionCategory::ModelConfig)
            );
        }
        other => panic!("expected SessionUpdate::ConfigOptionUpdate, got {other:?}"),
    }
}

#[test]
fn session_update_session_info_update_survives_round_trip() {
    let notification = SessionNotification::new(
        "s1",
        SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title("renamed session")),
    );

    let encoded = encode_notification("session/update", &notification).unwrap();
    let decoded: SessionNotification =
        decode_notification_params("session/update", &encoded.headers, &encoded.body).unwrap();

    match decoded.update {
        SessionUpdate::SessionInfoUpdate(update) => {
            assert_eq!(update.title.take(), Some("renamed session".to_string()));
        }
        other => panic!("expected SessionUpdate::SessionInfoUpdate, got {other:?}"),
    }
}

#[test]
fn session_update_content_chunk_message_id_survives_round_trip() {
    let notification = SessionNotification::new(
        "s1",
        SessionUpdate::AgentMessageChunk(
            ContentChunk::new(ContentBlock::from("hello")).message_id(MessageId::new("msg-1")),
        ),
    );

    let encoded = encode_notification("session/update", &notification).unwrap();
    let decoded: SessionNotification =
        decode_notification_params("session/update", &encoded.headers, &encoded.body).unwrap();

    match decoded.update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            assert_eq!(chunk.message_id, Some(MessageId::new("msg-1")));
        }
        other => panic!("expected SessionUpdate::AgentMessageChunk, got {other:?}"),
    }
}

#[test]
fn set_session_config_option_request_boolean_value_survives_round_trip() {
    let request = SetSessionConfigOptionRequest::new("s1", "verbose", SessionConfigOptionValue::boolean(true));

    let encoded = encode_request("session/set_config_option", RequestId::Number(1), &request).unwrap();
    let decoded: SetSessionConfigOptionRequest =
        decode_request_params("session/set_config_option", &encoded.headers, &encoded.body).unwrap();

    assert_eq!(decoded.config_id, SessionConfigId::new("verbose"));
    assert_eq!(decoded.value, SessionConfigOptionValue::boolean(true));
}

#[test]
fn set_session_config_option_request_model_config_category_option_survives_round_trip() {
    let option = SessionConfigOption::boolean("temperature", "Temperature", true)
        .category(SessionConfigOptionCategory::ModelConfig);
    let request = SetSessionConfigOptionRequest::new("s1", option.id.clone(), SessionConfigOptionValue::boolean(true));

    let encoded = encode_request("session/set_config_option", RequestId::Number(1), &request).unwrap();
    let decoded: SetSessionConfigOptionRequest =
        decode_request_params("session/set_config_option", &encoded.headers, &encoded.body).unwrap();

    assert_eq!(decoded.config_id, SessionConfigId::new("temperature"));
    assert_eq!(option.category, Some(SessionConfigOptionCategory::ModelConfig));
}
