use super::{SubjectPrincipalId, SubjectPrincipalIdError};
use crate::principal_id::PrincipalId;

const ALICE_SMITH_TOKEN: &str = "~Y0JYJ1ex_-CHcFaLBELjktfaZwWDWcpQ-clwcFTtaCM";

fn principal_id(value: &str) -> PrincipalId {
    PrincipalId::new(value).expect("valid principal id")
}

fn mapped(value: &str) -> SubjectPrincipalId {
    SubjectPrincipalId::from(&principal_id(value))
}

#[test]
fn an_id_that_is_already_a_safe_token_is_kept_verbatim() {
    for id in [
        "alice",
        "caller_01",
        "user+tag@host",
        "spiffe:svc-1",
        "bob~2",
        "UBKQ4OYEQT3ICSKDKMXJRLQ2V3PGR6NY4VLZ7ZJIBKI7NMNYHQSJ4WNM",
    ] {
        let token = mapped(id);

        assert_eq!(token.as_str(), id);
        assert!(!token.is_hashed());
    }
}

#[test]
fn an_id_that_is_not_a_safe_token_is_hashed_to_the_same_token_every_time() {
    let cases = [
        ("alice.smith", ALICE_SMITH_TOKEN),
        ("josé", "~2ZTh0AGIb-W0WxJnvR-it1KsUHQleb09rXsqKqDtaGY"),
        ("~alice", "~Z8IwEWYVR_qyZPm8VvnIUcFReP9O5uPAuCqOmAVghz8"),
        ("svc *", "~bZ80redvfHXrmZJsSUNyh_6wB0lU156vDyBUtTuNgqU"),
        (
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "~Y1NhxIu56rFBmOduqKt_GkFoXWrWKqkUbTAdTxfrCuA",
        ),
    ];

    for (id, expected) in cases {
        let token = mapped(id);

        assert_eq!(token.as_str(), expected, "{id:?} must map to its pinned token");
        assert!(token.is_hashed());
        assert_eq!(mapped(id), token);
    }
}

#[test]
fn every_character_a_subject_token_cannot_carry_forces_the_hashed_form() {
    for id in ["a.b", "a*b", "*", ">", "a>b", "a b", "a\u{a0}b", "user@example.com"] {
        assert!(mapped(id).is_hashed(), "{id:?} must be hashed");
    }
}

#[test]
fn the_longest_verbatim_id_is_sixty_four_characters() {
    let longest = "a".repeat(64);
    let too_long = "a".repeat(65);

    assert_eq!(mapped(&longest).as_str(), longest);
    assert!(mapped(&too_long).is_hashed());
}

#[test]
fn mapping_an_id_already_in_subject_form_returns_it_unchanged() {
    for id in ["alice", "alice.smith", "josé"] {
        let once = mapped(id);
        let twice = SubjectPrincipalId::from(once.principal_id());

        assert_eq!(twice, once, "{id:?} must not be mapped twice");
    }
}

#[test]
fn every_mapped_token_parses_back_to_itself() {
    for id in ["alice", "alice.smith", "~alice", "josé"] {
        let token = mapped(id);

        assert_eq!(SubjectPrincipalId::from_token(token.as_str()), Ok(token.clone()));
        assert_eq!(token.as_str().parse::<SubjectPrincipalId>(), Ok(token.clone()));
        assert_eq!(SubjectPrincipalId::try_from(token.as_str()), Ok(token.clone()));
        assert_eq!(token.to_string(), token.as_str());
        assert_eq!(token.principal_id().as_str(), token.as_str());
        assert_eq!(token.as_ref(), token.as_str());
    }
}

#[test]
fn an_empty_token_is_rejected() {
    assert_eq!(SubjectPrincipalId::from_token(""), Err(SubjectPrincipalIdError::Empty));
}

#[test]
fn a_wildcard_is_rejected_wherever_it_sits_in_the_token() {
    for (token, wildcard) in [("*", '*'), (">", '>'), ("a*", '*'), ("a>b", '>')] {
        assert_eq!(
            SubjectPrincipalId::from_token(token),
            Err(SubjectPrincipalIdError::InvalidCharacter(wildcard))
        );
    }
}

#[test]
fn a_token_that_would_split_the_subject_or_cannot_travel_on_it_is_rejected() {
    for (token, invalid) in [
        ("a.b", '.'),
        ("a b", ' '),
        ("a\tb", '\t'),
        ("a\u{0}b", '\u{0}'),
        ("josé", 'é'),
    ] {
        assert_eq!(
            SubjectPrincipalId::from_token(token),
            Err(SubjectPrincipalIdError::InvalidCharacter(invalid))
        );
    }
}

#[test]
fn a_token_longer_than_any_verbatim_id_is_rejected() {
    assert_eq!(
        SubjectPrincipalId::from_token(&"a".repeat(65)),
        Err(SubjectPrincipalIdError::TooLong)
    );
}

#[test]
fn a_hash_marked_token_that_is_not_a_whole_digest_is_rejected() {
    let truncated = &ALICE_SMITH_TOKEN[..ALICE_SMITH_TOKEN.len() - 1];
    let extended = format!("{ALICE_SMITH_TOKEN}A");
    let non_canonical = "~Y0JYJ1ex_-CHcFaLBELjktfaZwWDWcpQ-clwcFTtaCN";
    let wrong_alphabet = "~Y0JYJ1ex/+CHcFaLBELjktfaZwWDWcpQ+clwcFTtaCM";

    for token in [
        "~",
        "~alice",
        truncated,
        extended.as_str(),
        non_canonical,
        wrong_alphabet,
    ] {
        assert_eq!(
            SubjectPrincipalId::from_token(token),
            Err(SubjectPrincipalIdError::MalformedDigest),
            "{token:?} must not parse"
        );
    }
}

#[test]
fn a_credential_maps_exactly_like_a_derived_id() {
    for id in ["alice", "alice.smith", "~alice", "josé"] {
        assert_eq!(
            SubjectPrincipalId::from_credential(&principal_id(id)),
            Ok(mapped(id)),
            "{id:?} must map the same way at the edge"
        );
    }
}

#[test]
fn a_credential_whose_id_already_has_the_hashed_form_is_refused() {
    assert_eq!(
        SubjectPrincipalId::from_credential(&principal_id(ALICE_SMITH_TOKEN)),
        Err(SubjectPrincipalIdError::AlreadyHashed)
    );
}
