use acp_nats::wire::{decode_response, encode_agent_error};
use agent_client_protocol::Error;
use jsonrpc_nats::{Direction, Encoded, ResponseId, decode_value};
use serde_json::{Value, json};

#[test]
fn agent_errors_encode_infallibly_with_canonical_body_and_projections() -> Result<(), Box<dyn std::error::Error>> {
    let data = json!({"retryable": false, "details": {"values": [null, true, 42, "text"]}});
    let error = Error::new(-32042, "Runner refused\n\"request\"").data(data.clone());
    for id in [
        ResponseId::Null,
        ResponseId::Number(i64::MIN),
        ResponseId::String("request-\n\"é".into()),
    ] {
        let encoded: Encoded = encode_agent_error(id.clone(), &error);
        let expected = json!({
            "jsonrpc": "2.0",
            "id": id.to_json(),
            "error": {"code": -32042, "message": error.message, "data": data}
        });
        assert_eq!(serde_json::from_slice::<Value>(&encoded.body)?, expected);
        assert_eq!(
            decode_value(Direction::Response, None, &encoded.headers, &encoded.body)?,
            expected
        );
        let projected_id = (id != ResponseId::Null).then(|| id.to_json().to_string());
        assert_eq!(
            encoded.headers.get(jsonrpc_nats::HEADER_ID).map(|value| value.as_str()),
            projected_id.as_deref()
        );
        assert_eq!(
            encoded
                .headers
                .get(jsonrpc_nats::HEADER_ERROR_CODE)
                .map(|value| value.as_str()),
            Some("-32042")
        );
        let Err(recovered) = decode_response::<Value>(&encoded.headers, &encoded.body)? else {
            return Err(std::io::Error::other("an encoded agent error must decode as an error response").into());
        };
        assert_eq!(recovered.code, error.code);
        assert_eq!(recovered.message, error.message);
        assert_eq!(recovered.data, Some(data.clone()));
    }
    Ok(())
}
