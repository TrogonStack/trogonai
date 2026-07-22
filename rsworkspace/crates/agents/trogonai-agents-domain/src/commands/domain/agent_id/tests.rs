use super::*;

#[test]
fn rejects_empty() {
    let error = AgentId::parse("").unwrap_err();
    assert_eq!(error.violation, AgentIdViolation::Empty);
}

#[test]
fn rejects_surrounding_whitespace() {
    for raw in [" agent-1", "agent-1 ", "\tagent-1", "agent-1\n"] {
        let error = AgentId::parse(raw).unwrap_err();
        assert_eq!(error.violation, AgentIdViolation::SurroundingWhitespace, "{raw:?}");
    }
}

#[test]
fn accepts_valid_ids() {
    for raw in ["agent-1", "operator.agent42", "tenant_acme/agent"] {
        assert_eq!(AgentId::parse(raw).unwrap().as_str(), raw, "{raw:?}");
    }
}

#[test]
fn supports_standard_string_conversions_and_display() {
    let id = AgentId::parse("agent-1").unwrap();

    assert_eq!(id.as_ref(), "agent-1");
    assert_eq!("agent-1".parse::<AgentId>().unwrap().as_str(), "agent-1");
    assert_eq!(id.to_string(), "agent-1");
    assert_eq!(AgentIdViolation::Empty.to_string(), "must not be empty");
    assert_eq!(
        AgentIdViolation::SurroundingWhitespace.to_string(),
        "must not have leading or trailing whitespace"
    );
    assert_eq!(
        AgentId::parse("").unwrap_err().to_string(),
        "agent id '' is invalid: must not be empty"
    );
}
