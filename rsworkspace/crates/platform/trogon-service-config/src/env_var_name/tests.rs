use super::*;

#[test]
fn keeps_the_name_as_given() {
    let name = EnvVarName::new("NATS_TOKEN").unwrap();

    assert_eq!(name.as_str(), "NATS_TOKEN");
    assert_eq!(name.to_string(), "NATS_TOKEN");
}

#[test]
fn rejects_an_empty_name() {
    assert_eq!(EnvVarName::new(""), Err(EnvVarNameError::Empty));
}

#[test]
fn rejects_names_no_variable_can_hold() {
    for name in ["A=B", "A\0B"] {
        assert_eq!(
            EnvVarName::new(name),
            Err(EnvVarNameError::ReservedCharacter),
            "{name:?}"
        );
    }
}
