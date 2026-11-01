use super::{SubjectPrincipal, SubjectPrincipalError};
use crate::principal_id::PrincipalId;
use crate::principal_kind::{PrincipalKind, PrincipalKindError};
use crate::subject_principal_id::{SubjectPrincipalId, SubjectPrincipalIdError};

fn principal_id(value: &str) -> PrincipalId {
    PrincipalId::new(value).expect("valid principal id")
}

#[test]
fn a_principal_position_renders_as_its_kind_token_then_its_id_token() {
    let id = SubjectPrincipalId::from(&principal_id("caller_01"));
    let principal = SubjectPrincipal::new(PrincipalKind::Person, id);

    assert_eq!(principal.to_string(), "person.caller_01");
    assert_eq!(principal.kind(), PrincipalKind::Person);
    assert_eq!(principal.id().as_str(), "caller_01");
}

#[test]
fn a_principal_position_parses_back_from_the_tokens_it_renders() {
    for (kind, id) in [
        (PrincipalKind::Agent, "planner"),
        (PrincipalKind::Person, "alice.smith"),
        (PrincipalKind::Service, "scheduler worker"),
    ] {
        let principal = SubjectPrincipal::from_principal(kind, &principal_id(id));
        let rendered = principal.to_string();
        let (kind_token, id_token) = rendered.split_once('.').expect("two tokens");

        assert_eq!(SubjectPrincipal::from_tokens(kind_token, id_token), Ok(principal));
    }
}

#[test]
fn a_derived_principal_maps_back_to_the_subject_it_came_from() {
    for id in ["alice", "alice.smith"] {
        let principal = SubjectPrincipal::from_principal(PrincipalKind::Person, &principal_id(id));
        let derived = principal.principal_id();

        assert_eq!(derived.as_str(), principal.id().as_str());
        assert_eq!(SubjectPrincipal::from_principal(principal.kind(), &derived), principal);
    }
}

#[test]
fn an_unknown_or_empty_kind_token_names_no_principal() {
    assert_eq!(
        SubjectPrincipal::from_tokens("user", "alice"),
        Err(SubjectPrincipalError::Kind(PrincipalKindError::Unknown))
    );
    assert_eq!(
        SubjectPrincipal::from_tokens("Agent", "alice"),
        Err(SubjectPrincipalError::Kind(PrincipalKindError::Unknown))
    );
    assert_eq!(
        SubjectPrincipal::from_tokens("", "alice"),
        Err(SubjectPrincipalError::Kind(PrincipalKindError::Empty))
    );
}

#[test]
fn a_wildcard_kind_token_names_no_principal() {
    for kind in ["*", ">"] {
        assert_eq!(
            SubjectPrincipal::from_tokens(kind, "alice"),
            Err(SubjectPrincipalError::Kind(PrincipalKindError::Unknown))
        );
    }
}

#[test]
fn an_invalid_or_empty_id_token_names_no_principal() {
    assert_eq!(
        SubjectPrincipal::from_tokens("agent", ""),
        Err(SubjectPrincipalError::Id(SubjectPrincipalIdError::Empty))
    );
    assert_eq!(
        SubjectPrincipal::from_tokens("agent", "~alice"),
        Err(SubjectPrincipalError::Id(SubjectPrincipalIdError::MalformedDigest))
    );
    for (id, wildcard) in [("*", '*'), (">", '>')] {
        assert_eq!(
            SubjectPrincipal::from_tokens("agent", id),
            Err(SubjectPrincipalError::Id(SubjectPrincipalIdError::InvalidCharacter(
                wildcard
            )))
        );
    }
}

#[test]
fn a_credential_principal_lands_on_the_same_position_as_a_derived_one() {
    let id = principal_id("alice.smith");

    assert_eq!(
        SubjectPrincipal::from_credential(PrincipalKind::Person, &id),
        Ok(SubjectPrincipal::from_principal(PrincipalKind::Person, &id))
    );
}

#[test]
fn a_parsed_hashed_principal_maps_back_to_the_identical_token() {
    let token = SubjectPrincipalId::from(&principal_id("alice.smith")).to_string();
    let parsed = SubjectPrincipal::from_tokens("person", &token).expect("hashed token parses");

    let remapped = SubjectPrincipal::from_principal(parsed.kind(), &parsed.principal_id());

    assert_eq!(remapped.id().as_str(), token);
    assert_eq!(remapped.to_string(), format!("person.{token}"));
    assert_eq!(remapped, parsed);
}

#[test]
fn only_the_credential_edge_refuses_an_id_already_in_hashed_form() {
    let hashed = SubjectPrincipalId::from(&principal_id("alice.smith")).principal_id();

    assert_eq!(
        SubjectPrincipal::from_credential(PrincipalKind::Person, &hashed),
        Err(SubjectPrincipalError::Id(SubjectPrincipalIdError::AlreadyHashed))
    );
    assert_eq!(
        SubjectPrincipal::from_principal(PrincipalKind::Person, &hashed)
            .id()
            .as_str(),
        hashed.as_str()
    );
}
