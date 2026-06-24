use async_nats::header::HeaderMap;
use jsonrpc_nats::{Message, RequestId};

pub fn encode_wire_request<Req: serde::Serialize>(
    method: &str,
    id: RequestId,
    params: &Req,
) -> (HeaderMap, Vec<u8>) {
    let encoded = jsonrpc_nats::encode(&Message::Request {
        id,
        method: method.to_string(),
        params: serde_json::to_value(params).unwrap(),
    })
    .unwrap();
    (encoded.headers, encoded.body.to_vec())
}

pub fn encode_wire_notification<Req: serde::Serialize>(method: &str, params: &Req) -> (HeaderMap, Vec<u8>) {
    let encoded = jsonrpc_nats::encode(&Message::Notification {
        method: method.to_string(),
        params: serde_json::to_value(params).unwrap(),
    })
    .unwrap();
    (encoded.headers, encoded.body.to_vec())
}
