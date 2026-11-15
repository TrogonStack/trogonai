//! Replays the recorded inbound router tables (`pure.inbound` and `pure.uuid_set` of the goldens, written by
//! `.config/mise/lib/claude-sdk-freshness/bridge_worker_goldens.mjs` from the bundle's own `J4`, `ZPe`, `XPe` and
//! `If`, with `XPe` validating through the bundle's own `YPe` schema) through [`InboundRouter`] and [`UuidSet`].

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{INBOUND_DEDUP_CAPACITY, Inbound, InboundRouter, UuidSet};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDENS: &str = include_str!("fixtures/bridge_worker/goldens.json");

fn goldens() -> Result<Value, serde_json::Error> {
    serde_json::from_str(GOLDENS)
}

fn list<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing {key} in {value}"))
}

fn kind(inbound: &Inbound) -> (&'static str, Value) {
    match inbound {
        Inbound::User(message) => ("user", message.clone()),
        Inbound::ControlResponse(message) => ("control_response", message.clone()),
        Inbound::ControlRequest(message) => ("control_request", message.clone()),
        Inbound::ControlCancel(id) => ("control_cancel", json!(id)),
        Inbound::QueuedNotification(message) => ("queued_notification", message.clone()),
    }
}

#[test]
fn the_dedup_capacity_is_the_recorded_one() -> TestResult {
    assert_eq!(2_000, INBOUND_DEDUP_CAPACITY);
    Ok(())
}

#[test]
fn every_recorded_line_is_routed_like_the_upstream_router() -> TestResult {
    let goldens = goldens()?;
    let cases = list(&goldens["pure"], "inbound")?;
    assert!(cases.len() >= 50, "only {} inbound cases", cases.len());
    for case in cases {
        let mut router = InboundRouter::new(case["buffer"] == true);
        for uuid in list(case, "echo")? {
            router.remember_echo(uuid.as_str().ok_or("echo uuid")?);
        }
        let mut routed: Vec<Value> = Vec::new();
        for line in list(case, "lines")? {
            if let Some(inbound) = router.route(line.as_str().ok_or("line")?) {
                let (name, value) = kind(&inbound);
                routed.push(json!([name, value]));
            }
            routed.push(json!(["next"]));
        }
        assert_eq!(list(case, "out")?, &routed, "lines {}", case["lines"]);
    }
    Ok(())
}

#[test]
fn the_bounded_uuid_set_forgets_the_oldest_member_like_the_upstream_set() -> TestResult {
    let goldens = goldens()?;
    let cases = list(&goldens["pure"], "uuid_set")?;
    assert_eq!(3, cases.len());
    for case in cases {
        let capacity = usize::try_from(case["capacity"].as_u64().ok_or("capacity")?)?;
        let mut set = UuidSet::new(capacity);
        let results = list(case, "results")?;
        for (position, operation) in list(case, "ops")?.iter().enumerate() {
            let name = operation[0].as_str().ok_or("op")?;
            let uuid = operation[1].as_str().ok_or("uuid")?;
            let observed = match name {
                "add" => {
                    set.add(uuid);
                    Value::Null
                }
                "clear" => {
                    set.clear();
                    Value::Null
                }
                _ => json!(set.has(uuid)),
            };
            assert_eq!(
                results[position], observed,
                "op {position} ({name} {uuid}) at capacity {capacity}"
            );
        }
    }
    Ok(())
}

#[test]
fn a_write_the_host_made_is_remembered_once() {
    let mut router = InboundRouter::default();
    assert!(router.remember_echo("u1"));
    assert!(!router.remember_echo("u1"));
    assert!(router.is_echo("u1"));
    assert!(!router.is_echo("u2"));
}
