use bytes::Bytes;
use serde_json::{Value, json};
use trogon_sandbox_client::EvidenceFrame;

use super::EvidenceEventBody;

fn body(payload: &'static [u8]) -> Result<Value, serde_json::Error> {
    serde_json::to_value(EvidenceEventBody::from(EvidenceFrame {
        subject_suffix: "lifecycle".to_string(),
        payload: Bytes::from_static(payload),
    }))
}

#[test]
fn text_payloads_travel_as_they_are() -> Result<(), serde_json::Error> {
    assert_eq!(
        body(b"started")?,
        json!({"subject_suffix": "lifecycle", "payload": "started"})
    );
    Ok(())
}

#[test]
fn binary_payloads_travel_as_prefixed_base64() -> Result<(), serde_json::Error> {
    assert_eq!(
        body(&[0xff, 0xfe])?,
        json!({"subject_suffix": "lifecycle", "payload": "base64://4="})
    );
    Ok(())
}
