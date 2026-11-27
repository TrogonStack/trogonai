use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn ack(request_id: u64) -> String {
    format!(
        r#"{{"version":1,"execution_id":"e","nonce":"n","request_id":{request_id},"operation":"signal","signal":15,"delivered":true,"bytes_written":0,"eof":false}}"#
    )
}

#[test]
fn an_acknowledgement_for_this_execution_is_bound() -> TestResult {
    let parsed: OrdinaryAcknowledgement = serde_json::from_str(&ack(3))?;
    assert!(parsed.is_bound("e", "n", 3));
    assert!(parsed.is_bound("e", "n", 9));
    Ok(())
}

#[test]
fn a_foreign_or_future_acknowledgement_is_not_bound() -> TestResult {
    let parsed: OrdinaryAcknowledgement = serde_json::from_str(&ack(3))?;
    assert!(!parsed.is_bound("other", "n", 3));
    assert!(!parsed.is_bound("e", "other", 3));
    assert!(!parsed.is_bound("e", "n", 2));
    Ok(())
}

#[test]
fn unknown_fields_are_refused() {
    let padded = ack(1).replace('}', r#","extra":1}"#);
    assert!(serde_json::from_str::<OrdinaryAcknowledgement>(&padded).is_err());
}

#[test]
fn a_plain_acknowledgement_reports_no_delivery_bytes_or_end_of_input() -> TestResult {
    let plain: OrdinaryAcknowledgement = serde_json::from_str(
        r#"{"version":1,"execution_id":"e","nonce":"n","request_id":1,"operation":"launch","signal":null,"delivered":null,"bytes_written":0,"eof":false}"#,
    )?;
    assert!(plain.is_plain());
    let signal: OrdinaryAcknowledgement = serde_json::from_str(&ack(1))?;
    assert!(!signal.is_plain());
    Ok(())
}
