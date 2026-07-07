use super::*;
use agent_client_protocol::{
    ClientCapabilities, ClientNesCapabilities, ElicitationCapabilities, ElicitationFormCapabilities, Error,
    InitializeRequest, InitializeResponse, LoadSessionRequest, NesCapabilities, NewSessionRequest,
    PositionEncodingKind, ProtocolVersion,
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
        agent_client_protocol::AgentCapabilities::new()
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
