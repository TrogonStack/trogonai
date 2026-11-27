use super::*;

#[test]
fn the_command_id_is_the_key_and_the_command_is_the_fallback() {
    let by_id = CommandAttribution::for_command("echo hi", Some("0123456789abcdefghijklmn"));
    assert_eq!(
        by_id.proxy_user().raw(),
        format!("sandbox.{}", STANDARD.encode("0123456789abcdefghijklmn"))
    );
    for id in [None, Some("")] {
        let by_command = CommandAttribution::for_command("echo hi", id);
        assert_eq!(by_command.proxy_user().raw(), "sandbox.ZWNobyBoaQ==");
    }
}

#[test]
fn nothing_to_attribute_is_the_bare_user() {
    assert_eq!(CommandAttribution::default().proxy_user().raw(), "sandbox");
    assert_eq!(CommandAttribution::new("").proxy_user().raw(), "sandbox");
}

#[test]
fn a_key_is_base64_after_the_user_name() {
    let user = CommandAttribution::new("echo hi").proxy_user();
    assert_eq!(user.raw(), "sandbox.ZWNobyBoaQ==");
    assert_eq!(user.in_url(), "sandbox.ZWNobyBoaQ%3D%3D");
}

#[test]
fn url_encoding_keeps_the_unreserved_marks() {
    let user = ProxyUser("sandbox.a+b/c=d-_.!~*'()".to_string());
    assert_eq!(user.in_url(), "sandbox.a%2Bb%2Fc%3Dd-_.!~*'()");
}

#[test]
fn only_the_first_hundred_code_units_count() {
    let long = "a".repeat(150);
    let user = CommandAttribution::new(&long).proxy_user();
    assert_eq!(user.raw(), format!("sandbox.{}", STANDARD.encode("a".repeat(100))));
}

#[test]
fn a_surrogate_pair_cut_in_the_middle_becomes_a_replacement_character() {
    let key = format!("{}\u{1F600}", "a".repeat(99));
    let user = CommandAttribution::new(&key).proxy_user();
    let expected = format!("{}\u{FFFD}", "a".repeat(99));
    assert_eq!(user.raw(), format!("sandbox.{}", STANDARD.encode(expected)));
}

#[test]
fn a_pair_that_fits_is_kept() {
    let key = format!("{}\u{1F600}", "a".repeat(98));
    let user = CommandAttribution::new(&key).proxy_user();
    assert_eq!(user.raw(), format!("sandbox.{}", STANDARD.encode(key)));
}

#[test]
fn a_name_past_the_socks_budget_falls_back_to_the_bare_user() {
    let key = "\u{20AC}".repeat(100);
    assert_eq!(CommandAttribution::new(&key).proxy_user().raw(), "sandbox");
}
