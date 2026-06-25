use super::*;
use agent_client_protocol::Error;

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
