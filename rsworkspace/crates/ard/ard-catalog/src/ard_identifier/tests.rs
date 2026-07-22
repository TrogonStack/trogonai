use super::*;

#[test]
fn accepts_valid_urn_air() {
    let id = ArdIdentifier::new("urn:air:example.com:agent:assistant").unwrap();
    assert_eq!(id.as_str(), "urn:air:example.com:agent:assistant");
    assert_eq!(id.publisher_domain(), "example.com");
    assert_eq!(id.resource_name(), "assistant");
}

#[test]
fn accepts_agent_localhost_publisher() {
    let id = ArdIdentifier::new("urn:air:agent.localhost:demo:helper").unwrap();
    assert_eq!(id.publisher_domain(), "agent.localhost");
}

#[test]
fn accepts_identifier_without_namespace() {
    let id = ArdIdentifier::new("urn:air:example.com:assistant").unwrap();
    assert_eq!(id.publisher_domain(), "example.com");
    assert_eq!(id.resource_name(), "assistant");
}

#[test]
fn rejects_empty() {
    assert_eq!(ArdIdentifier::new(""), Err(ArdIdentifierError::Empty));
}

#[test]
fn rejects_urn_ai_prefix() {
    assert_eq!(
        ArdIdentifier::new("urn:ai:example.com:agent:assistant"),
        Err(ArdIdentifierError::DeprecatedAiPrefix)
    );
    assert_eq!(
        ArdIdentifier::new("urn:ai"),
        Err(ArdIdentifierError::DeprecatedAiPrefix)
    );
}

#[test]
fn rejects_invalid_prefix() {
    assert_eq!(
        ArdIdentifier::new("urn:foo:example.com:agent:assistant"),
        Err(ArdIdentifierError::InvalidPrefix)
    );
}

#[test]
fn rejects_too_few_components() {
    assert_eq!(
        ArdIdentifier::new("urn:air:example.com"),
        Err(ArdIdentifierError::TooFewComponents)
    );
    assert_eq!(
        ArdIdentifier::new("urn:air:"),
        Err(ArdIdentifierError::TooFewComponents)
    );
}

#[test]
fn rejects_invalid_identifier_characters() {
    assert_eq!(
        ArdIdentifier::new("urn:air:example com:agent"),
        Err(ArdIdentifierError::InvalidPublisherCharacter(' '))
    );
    assert_eq!(
        ArdIdentifier::new("urn:air:example_com:agent"),
        Err(ArdIdentifierError::InvalidPublisherCharacter('_'))
    );
    assert_eq!(
        ArdIdentifier::new("urn:air:example.com:agent/tool"),
        Err(ArdIdentifierError::InvalidResourceCharacter('/'))
    );
}

#[test]
fn rejects_empty_dns_label() {
    assert!(matches!(
        ArdIdentifier::new("urn:air:example..com:agent:x"),
        Err(ArdIdentifierError::InvalidPublisherLabel(_))
    ));
}

#[test]
fn rejects_leading_hyphen_in_dns_label() {
    assert!(matches!(
        ArdIdentifier::new("urn:air:-example.com:agent:x"),
        Err(ArdIdentifierError::InvalidPublisherLabel(_))
    ));
}

#[test]
fn rejects_trailing_hyphen_in_dns_label() {
    assert!(matches!(
        ArdIdentifier::new("urn:air:example-.com:agent:x"),
        Err(ArdIdentifierError::InvalidPublisherLabel(_))
    ));
}

#[test]
fn accepts_valid_multi_label_domain() {
    let id = ArdIdentifier::new("urn:air:sub.example.com:agent:assistant").unwrap();
    assert_eq!(id.publisher_domain(), "sub.example.com");
}
