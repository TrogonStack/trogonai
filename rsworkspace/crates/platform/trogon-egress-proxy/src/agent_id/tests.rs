use super::*;

#[test]
fn accepts_url_safe_identifiers() {
    for raw in ["agent-1", "a", "A_b.c-9", "0123456789abcdef", "did:key:z6MkhaXgBZDvotDkL5257faiztiGi2sfKfzvuU5TVndZqquw"] {
        assert_eq!(AgentId::new(raw).expect("valid id").as_str(), raw);
    }
}

#[test]
fn rejects_empty() {
    assert_eq!(AgentId::new(""), Err(AgentIdError::Empty));
}

#[test]
fn rejects_over_long() {
    assert_eq!(AgentId::new("a".repeat(129)), Err(AgentIdError::TooLong { max: 128 }));
    assert!(AgentId::new("a".repeat(128)).is_ok());
}

#[test]
fn rejects_characters_that_would_need_uri_escaping() {
    for raw in ["a/b", "a b", "a?b", "a#b", "a%41", "a@b", "é", "a\n"] {
        assert!(
            matches!(AgentId::new(raw), Err(AgentIdError::InvalidCharacter { .. })),
            "{raw:?} must be rejected"
        );
    }
}
