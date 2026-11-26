use super::*;

#[test]
fn the_default_is_enforce() {
    assert_eq!(EnforcementMode::default(), EnforcementMode::Enforce);
}

#[test]
fn both_modes_parse_from_their_exact_names() {
    assert_eq!("enforce".parse(), Ok(EnforcementMode::Enforce));
    assert_eq!("dry_run".parse(), Ok(EnforcementMode::DryRun));
    assert_eq!(EnforcementMode::DryRun.to_string(), "dry_run");
}

#[test]
fn anything_else_is_refused_rather_than_read_as_a_mode() {
    for value in [
        "", "Enforce", "DRY_RUN", "dry-run", "dryrun", " enforce", "off", "audit",
    ] {
        assert_eq!(value.parse::<EnforcementMode>(), Err(EnforcementModeError), "{value:?}");
    }
}
