use super::{PrincipalKind, PrincipalKindError};

#[test]
fn every_kind_parses_back_from_the_wire_name_it_prints() {
    for (kind, wire) in [
        (PrincipalKind::Agent, "agent"),
        (PrincipalKind::Person, "person"),
        (PrincipalKind::Service, "service"),
    ] {
        assert_eq!(kind.as_str(), wire);
        assert_eq!(kind.to_string(), wire);
        assert_eq!(wire.parse::<PrincipalKind>(), Ok(kind));
        assert_eq!(PrincipalKind::try_from(wire), Ok(kind));
    }
}

#[test]
fn an_empty_wire_name_names_no_kind() {
    assert_eq!("".parse::<PrincipalKind>(), Err(PrincipalKindError::Empty));
}

#[test]
fn a_wire_name_outside_the_closed_set_names_no_kind() {
    for token in ["Agent", "PERSON", "user", "services", "*", ">", "agent ", " agent"] {
        assert_eq!(
            token.parse::<PrincipalKind>(),
            Err(PrincipalKindError::Unknown),
            "{token:?} must not parse"
        );
    }
}
