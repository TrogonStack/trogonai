use super::*;

#[test]
fn accepts_a_canonical_published_subject() {
    assert_eq!(validate_published_subject("acp.v1.session.sess_1.agent.prompt"), Ok(()));
}

#[test]
fn accepts_opaque_identifiers_in_dynamic_positions() {
    // A published subject carries values, not grammar, in its dynamic
    // positions. Rejecting these would reject every real session id.
    for subject in [
        "acp.v1.session.SeSs-AbC123.agent.prompt",
        "a2a.v1.tasks.019706af7ffd7c2e8b1a4c5d6e7f8091.events",
        "acp.v1.session.sess.1.agent.prompt",
    ] {
        assert_eq!(validate_published_subject(subject), Ok(()), "{subject}");
    }
}

#[test]
fn rejects_wildcards_in_a_published_subject() {
    assert_eq!(
        validate_published_subject("acp.v1.session.*.agent.prompt"),
        Err(SubjectViolationError::WildcardInPublishedSubject { index: 3 })
    );
    assert_eq!(
        validate_published_subject("acp.v1.session.sess_1.agent.>"),
        Err(SubjectViolationError::WildcardInPublishedSubject { index: 5 })
    );
}

#[test]
fn accepts_canonical_patterns() {
    for subject in [
        "acp.v1.session.*.agent.response",
        "acp.v1.session.*.client.>",
        "a2a.v1.tasks.*.events",
        "acp.v1.global.agent.session.new",
    ] {
        assert_eq!(validate_subject_pattern(subject), Ok(()), "{subject}");
    }
}

#[test]
fn rejects_a_trailing_wildcard_that_is_not_last() {
    assert_eq!(
        validate_subject_pattern("acp.v1.>.agent.prompt"),
        Err(SubjectViolationError::TrailingWildcardNotLast { index: 2 })
    );
}

#[test]
fn rejects_non_lower_snake_grammar_in_a_pattern() {
    assert_eq!(
        validate_subject_pattern("acp.v1.session.*.agent.setMode"),
        Err(SubjectViolationError::NotLowerSnake {
            index: 5,
            token: "setMode".to_owned()
        })
    );
    assert_eq!(
        validate_subject_pattern("acp.v1.session.*.agent.set-mode"),
        Err(SubjectViolationError::NotLowerSnake {
            index: 5,
            token: "set-mode".to_owned()
        })
    );
}

#[test]
fn exempts_the_token_following_the_escape_marker() {
    // ADR#0055's `custom.{base64url}` escape is the sole lower_snake exemption.
    assert_eq!(validate_subject_pattern("mcp.v1.tools.custom.dG9vbHMvY2FsbA"), Ok(()));
    // The exemption covers exactly one token, not the rest of the subject.
    assert_eq!(
        validate_subject_pattern("mcp.v1.tools.custom.dG9vbHM.Nope"),
        Err(SubjectViolationError::NotLowerSnake {
            index: 5,
            token: "Nope".to_owned()
        })
    );
}

#[test]
fn escape_exemption_does_not_waive_the_wildcard_rule() {
    assert_eq!(
        validate_published_subject("mcp.v1.tools.custom.*"),
        Err(SubjectViolationError::WildcardInPublishedSubject { index: 4 })
    );
}

#[test]
fn rejects_a_flat_subject() {
    assert_eq!(validate_published_subject("acp"), Err(SubjectViolationError::Flat));
}

#[test]
fn rejects_an_empty_subject() {
    assert_eq!(validate_published_subject(""), Err(SubjectViolationError::Empty));
}

#[test]
fn rejects_malformed_dots() {
    for subject in ["acp..v1.session", ".acp.v1.session", "acp.v1.session."] {
        assert_eq!(
            validate_published_subject(subject),
            Err(SubjectViolationError::MalformedDots),
            "{subject}"
        );
        assert_eq!(
            validate_subject_pattern(subject),
            Err(SubjectViolationError::MalformedDots),
            "{subject}"
        );
    }
}

#[test]
fn enforces_the_sixteen_token_budget() {
    let ok = vec!["t"; MAX_SUBJECT_TOKENS].join(".");
    assert_eq!(validate_published_subject(&ok), Ok(()));

    let over = vec!["t"; MAX_SUBJECT_TOKENS + 1].join(".");
    assert_eq!(
        validate_published_subject(&over),
        Err(SubjectViolationError::TooManyTokens {
            count: MAX_SUBJECT_TOKENS + 1
        })
    );
}

#[test]
fn enforces_the_two_hundred_fifty_six_byte_budget() {
    let ok = format!("acp.v1.session.{}", "a".repeat(MAX_SUBJECT_BYTES - 15));
    assert_eq!(ok.len(), MAX_SUBJECT_BYTES);
    assert_eq!(validate_published_subject(&ok), Ok(()));

    let over = format!("{ok}a");
    assert_eq!(
        validate_published_subject(&over),
        Err(SubjectViolationError::TooLong {
            bytes: MAX_SUBJECT_BYTES + 1
        })
    );
}

#[test]
fn rejects_whitespace_and_partial_wildcards() {
    assert_eq!(
        validate_published_subject("acp.v1.session.a b.agent.prompt"),
        Err(SubjectViolationError::Token {
            index: 3,
            token: "a b".to_owned(),
            source: SubjectTokenViolationError::InvalidCharacter(' ')
        })
    );
    assert_eq!(
        validate_subject_pattern("acp.v1.session.a*.agent.prompt"),
        Err(SubjectViolationError::PartialWildcard {
            index: 3,
            token: "a*".to_owned()
        })
    );
}

#[test]
fn binding_version_follows_a_dotted_prefix() {
    assert_eq!(validate_binding_version("acp.v1.session.s.agent.prompt", "acp"), Ok(()));
    assert_eq!(
        validate_binding_version("my.multi.part.v1.session.s.agent.prompt", "my.multi.part"),
        Ok(())
    );
    assert_eq!(
        validate_binding_version("acp.v12.global.agent.initialize", "acp"),
        Ok(())
    );
}

#[test]
fn rejects_a_missing_binding_version() {
    assert_eq!(
        validate_binding_version("acp.session.s.agent.prompt", "acp"),
        Err(SubjectViolationError::MissingBindingVersion {
            found: Some("session".to_owned())
        })
    );
    assert_eq!(
        validate_binding_version("acp.version1.session", "acp"),
        Err(SubjectViolationError::MissingBindingVersion {
            found: Some("version1".to_owned())
        })
    );
}

#[test]
fn rejects_a_prefix_that_does_not_match() {
    // `acp2` must not satisfy a check against prefix `acp`.
    assert_eq!(
        validate_binding_version("acp2.v1.session.s", "acp"),
        Err(SubjectViolationError::PrefixMismatch {
            prefix: "acp".to_owned()
        })
    );
}

#[test]
fn detects_both_request_id_shapes() {
    // Hyphenated, as `acp-nats` mints them.
    assert!(looks_like_request_id("019706af-7ffd-7c2e-8b1a-4c5d6e7f8091"));
    // Simple 32-hex, as `a2a-nats` mints them.
    assert!(looks_like_request_id("019706af7ffd7c2e8b1a4c5d6e7f8091"));

    for token in ["session", "req_1", "sess-1", "v1", "", "019706af7ffd7c2e8b1a4c5d6e7f80"] {
        assert!(!looks_like_request_id(token), "{token}");
    }
}

#[test]
fn rejects_a_request_id_token_in_a_subject() {
    assert_eq!(
        validate_no_request_id_tokens("acp.v1.session.s.agent.response.019706af-7ffd-7c2e-8b1a-4c5d6e7f8091"),
        Err(SubjectViolationError::RequestIdToken {
            index: 6,
            token: "019706af-7ffd-7c2e-8b1a-4c5d6e7f8091".to_owned()
        })
    );
    assert_eq!(
        validate_no_request_id_tokens("acp.v1.session.s.agent.response.>"),
        Ok(())
    );
}
