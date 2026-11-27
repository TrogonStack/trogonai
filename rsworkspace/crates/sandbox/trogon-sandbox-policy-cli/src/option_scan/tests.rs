use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_missing_value_is_named_with_the_option_flags() {
    assert_eq!(
        missing_argument("-s, --settings <path>"),
        Exit::Error("error: option '-s, --settings <path>' argument missing\n".to_string())
    );
}

#[test]
fn the_closest_candidate_is_suggested() {
    assert_eq!(
        suggest_similar("--sandbox-usr", &["--sandbox-user", "--force"]),
        "\n(Did you mean --sandbox-user?)"
    );
}

#[test]
fn equally_close_candidates_are_listed_sorted() {
    assert_eq!(
        suggest_similar("cat", &["cap", "car"]),
        "\n(Did you mean one of cap, car?)"
    );
}

#[test]
fn nothing_close_suggests_nothing() {
    assert_eq!(suggest_similar("--zzzzzzzz", &["--force"]), "");
}

#[test]
fn a_value_attached_with_an_equals_sign_is_taken() -> TestResult {
    let specs = [Spec {
        id: 'v',
        short: Some("-v"),
        long: Some("--value"),
        flags: "-v, --value <x>",
        takes_value: true,
    }];
    let mut seen = Vec::new();
    let split = parse_options(&specs, &["--value=a=b".to_string(), "op".to_string()], |spec, value| {
        seen.push((spec.id, value));
        Ok(())
    })
    .map_err(|exit| format!("{exit:?}"))?;
    assert_eq!(seen, [('v', Some("a=b".to_string()))]);
    assert_eq!(split.operands, ["op"]);
    Ok(())
}

#[test]
fn everything_after_the_first_unknown_option_belongs_to_the_command() -> TestResult {
    let specs = [Spec {
        id: 'd',
        short: Some("-d"),
        long: None,
        flags: "-d",
        takes_value: false,
    }];
    let words: Vec<String> = ["ls", "-la", "-d", "x"].map(String::from).into();
    let split = parse_options(&specs, &words, |_, _| Ok(())).map_err(|exit| format!("{exit:?}"))?;
    assert_eq!(split.operands, ["ls"]);
    assert_eq!(split.unknown, ["-la", "x"]);
    Ok(())
}
