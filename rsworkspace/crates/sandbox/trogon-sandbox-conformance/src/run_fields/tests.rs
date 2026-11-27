use std::error::Error;

use super::*;

#[test]
fn empty_fields_serialize_to_an_empty_object() -> Result<(), Box<dyn Error>> {
    assert_eq!(RunFields::default().to_string(), "{}");
    Ok(())
}

#[test]
fn set_fields_serialize_under_their_request_names() {
    assert_eq!(RunFields::timeout(3).to_string(), r#"{"timeout_seconds":3}"#);
    assert_eq!(RunFields::detached(true).to_string(), r#"{"detached":true}"#);
    assert_eq!(RunFields::cwd("/tmp").to_string(), r#"{"cwd":"/tmp"}"#);
    assert_eq!(RunFields::env("A", "b").to_string(), r#"{"env":{"A":"b"}}"#);
}

#[test]
fn a_process_spec_names_its_readiness_kind() -> Result<(), Box<dyn Error>> {
    let named = serde_json::to_string(&ProcessSpec::named("svc"))?;
    assert_eq!(named, r#"{"name":"svc"}"#);
    let output = serde_json::to_string(&ProcessSpec::ready_when(Readiness::OutputContains { needle: "up" }))?;
    assert_eq!(output, r#"{"readiness":{"kind":"output_contains","needle":"up"}}"#);
    let port = serde_json::to_string(&ProcessSpec::ready_when(Readiness::TcpPort { port: 80 }))?;
    assert_eq!(port, r#"{"readiness":{"kind":"tcp_port","port":80}}"#);
    Ok(())
}
