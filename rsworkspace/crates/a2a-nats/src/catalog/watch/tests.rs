use super::*;
use bytes::Bytes;
use time::OffsetDateTime;

fn minimal_card_json(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "description": "",
        "version": "",
        "defaultInputModes": [],
        "defaultOutputModes": [],
        "skills": [],
        "capabilities": {},
        "supportedInterfaces": [{
            "url": "https://example.com/a2a",
            "protocolBinding": "JSONRPC",
            "protocolVersion": "0.2.0"
        }]
    })
}

fn entry(key: &str, op: Operation, value: Bytes, revision: u64) -> kv::Entry {
    kv::Entry {
        bucket: "A2A_AGENT_CARDS".to_string(),
        key: key.to_string(),
        value,
        revision,
        delta: 0,
        created: OffsetDateTime::UNIX_EPOCH,
        operation: op,
        seen_current: true,
    }
}

#[test]
fn watch_error_display_covers_every_variant() {
    let bad = serde_json::from_str::<String>("x").unwrap_err();
    let schema = a2a_pack::validate_agent_card_value(&serde_json::json!({})).unwrap_err();
    let deserialize_expected = format!("failed to deserialize AgentCard: {bad}");
    let schema_expected = format!("AgentCard schema validation failed: {schema}");
    assert_eq!(
        AgentCardWatchError::Kv("down".into()).to_string(),
        "KV watch error: down"
    );
    assert_eq!(
        AgentCardWatchError::InvalidKey("bad".into()).to_string(),
        "invalid catalog key: bad"
    );
    assert_eq!(
        AgentCardWatchError::Deserialize(bad).to_string(),
        deserialize_expected
    );
    assert_eq!(
        AgentCardWatchError::Schema(schema).to_string(),
        schema_expected
    );
}

#[test]
fn map_kv_entry_put_valid_card() {
    let body = serde_json::to_vec(&minimal_card_json("bot")).unwrap();
    let entry = entry("bot", Operation::Put, Bytes::from(body), 7);

    let event = map_kv_entry(entry).expect("valid put should map");
    assert!(matches!(
        event,
        AgentCardWatchEvent::Put { ref agent_id, ref card, revision: 7 }
            if agent_id.as_str() == "bot" && card.name == "bot"
    ));
}

#[test]
fn map_kv_entry_put_invalid_json_returns_deserialize() {
    let entry = entry("bot", Operation::Put, Bytes::from_static(b"not json"), 1);
    let err = map_kv_entry(entry).expect_err("invalid json must error");
    assert!(matches!(err, AgentCardWatchError::Deserialize(_)));
}

#[test]
fn map_kv_entry_put_schema_failure_returns_schema() {
    let body = serde_json::to_vec(&serde_json::json!({})).unwrap();
    let entry = entry("bot", Operation::Put, Bytes::from(body), 1);
    let err = map_kv_entry(entry).expect_err("missing required fields must error");
    assert!(matches!(err, AgentCardWatchError::Schema(_)));
}

#[test]
fn map_kv_entry_delete_yields_delete_event() {
    let entry = entry("bot", Operation::Delete, Bytes::new(), 9);
    let event = map_kv_entry(entry).expect("delete should map");
    assert!(matches!(
        event,
        AgentCardWatchEvent::Delete { ref agent_id, revision: 9 } if agent_id.as_str() == "bot"
    ));
}

#[test]
fn map_kv_entry_purge_yields_delete_event() {
    let entry = entry("bot", Operation::Purge, Bytes::new(), 10);
    let event = map_kv_entry(entry).expect("purge should map to delete");
    assert!(matches!(event, AgentCardWatchEvent::Delete { revision: 10, .. }));
}

#[test]
fn map_kv_entry_invalid_key_returns_invalid_key() {
    let body = serde_json::to_vec(&minimal_card_json("bot")).unwrap();
    let entry = entry("not a valid id!", Operation::Put, Bytes::from(body), 1);
    let err = map_kv_entry(entry).expect_err("bogus key must error");
    assert!(matches!(err, AgentCardWatchError::InvalidKey(_)));
}
