use super::*;

fn reference(value: &str) -> ModuleReference {
    value.parse().expect("a well-formed reference")
}

#[test]
fn a_reference_round_trips_through_its_written_form() {
    let parsed = reference("scheduler.schedules@0.1.0");

    assert_eq!(parsed.name().as_str(), "scheduler.schedules");
    assert_eq!(parsed.version().as_str(), "0.1.0");
    assert_eq!(parsed.to_string(), "scheduler.schedules@0.1.0");
}

#[test]
fn the_object_key_swaps_the_one_character_object_names_reject() {
    assert_eq!(
        reference("scheduler.schedules@0.1.0").object_key(),
        "scheduler.schedules/0.1.0"
    );
}

#[test]
fn a_version_with_dots_stays_one_version() {
    let parsed = reference("a.b@1.2.3");

    assert_eq!(parsed.version().as_str(), "1.2.3");
    assert_eq!(parsed.object_key(), "a.b/1.2.3");
}

#[test]
fn a_reference_with_no_version_names_a_family_rather_than_a_build() {
    let error = "scheduler.schedules"
        .parse::<ModuleReference>()
        .expect_err("a family is not deployable");

    assert!(
        matches!(error, ModuleReferenceError::MissingVersion { .. }),
        "a host that guessed the version would roll a deployment forward on its own: {error}"
    );
}

#[test]
fn neither_half_may_be_empty() {
    assert!(matches!(
        "@0.1.0".parse::<ModuleReference>().expect_err("a nameless module"),
        ModuleReferenceError::Name { .. }
    ));
    assert!(matches!(
        "scheduler.schedules@"
            .parse::<ModuleReference>()
            .expect_err("a versionless module"),
        ModuleReferenceError::Version { .. }
    ));
}

#[test]
fn the_separator_cannot_appear_twice() {
    let error = "a@b@c".parse::<ModuleReference>().expect_err("an ambiguous reference");

    assert!(
        matches!(error, ModuleReferenceError::Version { .. }),
        "splitting on the first separator must leave a version that rejects the second: {error}"
    );
}

#[test]
fn an_object_key_is_not_a_reference() {
    let error = "scheduler.schedules/0.1.0"
        .parse::<ModuleReference>()
        .expect_err("the key form is not the written form");

    assert!(
        matches!(error, ModuleReferenceError::MissingVersion { .. }),
        "the two renderings stay distinguishable so neither can be pasted where the other belongs: {error}"
    );
}

#[test]
fn the_file_name_is_the_written_form() {
    assert_eq!(
        reference("scheduler.schedules@0.1.0").file_name(),
        "scheduler.schedules@0.1.0.wasm"
    );
}
