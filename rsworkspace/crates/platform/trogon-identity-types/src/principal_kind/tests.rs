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
fn every_kind_travels_in_json_as_its_wire_name() {
    for kind in [PrincipalKind::Agent, PrincipalKind::Person, PrincipalKind::Service] {
        let json = serde_json::to_value(kind).expect("serialize kind");

        assert_eq!(json, serde_json::Value::String(kind.as_str().to_owned()));
        assert_eq!(
            serde_json::from_value::<PrincipalKind>(json).expect("deserialize kind"),
            kind
        );
    }
}

#[test]
fn a_json_value_that_is_not_a_known_wire_name_names_no_kind() {
    for json in [
        serde_json::json!("user"),
        serde_json::json!(""),
        serde_json::json!("Person"),
        serde_json::json!(1),
        serde_json::json!(null),
    ] {
        assert!(
            serde_json::from_value::<PrincipalKind>(json.clone()).is_err(),
            "{json} must not deserialize"
        );
    }
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
