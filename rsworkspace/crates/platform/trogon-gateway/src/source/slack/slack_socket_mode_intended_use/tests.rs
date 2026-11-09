use super::*;

#[test]
fn every_intended_use_parses_back_from_its_display_form() {
    for intended_use in [
        SlackSocketModeIntendedUse::Development,
        SlackSocketModeIntendedUse::BehindFirewall,
    ] {
        assert_eq!(intended_use.to_string().parse(), Ok(intended_use));
    }
}

#[test]
fn production_is_not_an_intended_use() {
    assert_eq!(
        "production".parse::<SlackSocketModeIntendedUse>(),
        Err(SlackSocketModeIntendedUseError::Unsupported("production".to_string()))
    );
}

#[test]
fn parsing_is_exact() {
    assert!("Development".parse::<SlackSocketModeIntendedUse>().is_err());
    assert!(" development".parse::<SlackSocketModeIntendedUse>().is_err());
}
