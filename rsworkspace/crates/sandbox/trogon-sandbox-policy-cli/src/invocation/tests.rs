use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Json = serde_json::Value;

const EXPECTED: &str = include_str!("../../tests/fixtures/option-scan/expected.json");

fn field<'a>(value: &'a Json, key: &str) -> Result<&'a Json, Box<dyn std::error::Error>> {
    value.get(key).ok_or_else(|| format!("missing {key}").into())
}

fn text<'a>(value: &'a Json, key: &str) -> Result<&'a str, Box<dyn std::error::Error>> {
    field(value, key)?
        .as_str()
        .ok_or_else(|| format!("{key} is not a string").into())
}

fn strings(value: &Json) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    value
        .as_array()
        .ok_or("not an array")?
        .iter()
        .map(|item| Ok(item.as_str().ok_or("not a string")?.to_string()))
        .collect()
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

#[test]
fn parses_every_recorded_argument_list() -> TestResult {
    let cases: Vec<Json> = serde_json::from_str(EXPECTED)?;
    assert!(!cases.is_empty());
    for case in cases {
        let argv = strings(field(&case, "argv")?)?;
        let outcome = field(&case, "outcome")?;
        let got = Invocation::parse(&argv, "0.0.0");
        let context = format!("{argv:?}");
        match text(outcome, "kind")? {
            "run" => {
                let Invocation::Run(args) = got else {
                    return Err(format!("{context}: {got:?}").into());
                };
                assert_eq!(args.command, strings(field(outcome, "args")?)?, "{context}");
                assert_eq!(Some(args.debug), field(outcome, "debug")?.as_bool(), "{context}");
                assert_eq!(
                    args.guest_view,
                    outcome.get("guestView").and_then(Json::as_bool).unwrap_or(false),
                    "{context}"
                );
                assert_eq!(
                    args.settings.as_ref().map(|path| path.to_string_lossy().into_owned()),
                    field(outcome, "settings")?.as_str().map(String::from),
                    "{context}"
                );
                assert_eq!(
                    args.command_string.as_deref(),
                    field(outcome, "c")?.as_str(),
                    "{context}"
                );
                assert_eq!(
                    args.control_fd.map(|fd| i64::from(fd.raw())),
                    field(outcome, "controlFd")?.as_i64(),
                    "{context}"
                );
            }
            "proxy" => {
                assert_eq!(got, Invocation::Proxy(strings(field(outcome, "args")?)?), "{context}");
            }
            kind @ ("windows-install" | "windows-uninstall") => {
                #[cfg(windows)]
                assert!(
                    matches!(&got, Invocation::Windows { uninstall, .. } if *uninstall == (kind == "windows-uninstall")),
                    "{context}: {got:?}"
                );
                #[cfg(not(windows))]
                {
                    let Invocation::Fail(text) = got else {
                        return Err(format!("{context}: {got:?}").into());
                    };
                    assert!(text.contains(kind), "{context}: {text}");
                }
            }
            "exit" => {
                let stdout = text(outcome, "stdout")?;
                let stderr = text(outcome, "stderr")?;
                match field(outcome, "code")?.as_i64() {
                    Some(0) => assert_eq!(got, Invocation::Print(stdout.to_string()), "{context}"),
                    _ => assert_eq!(got, Invocation::Fail(stderr.to_string()), "{context}"),
                }
            }
            other => return Err(format!("unknown kind {other}").into()),
        }
    }
    Ok(())
}

#[test]
fn the_guest_view_flag_is_taken_before_the_first_unknown_option() -> TestResult {
    let Invocation::Run(args) = Invocation::parse(&argv(&["--guest-view", "echo"]), "0.0.0") else {
        return Err("not a run".into());
    };
    assert!(args.guest_view);
    assert_eq!(args.command, ["echo"]);
    Ok(())
}

#[test]
fn the_version_flag_prints_the_version() {
    assert_eq!(
        Invocation::parse(&argv(&["--version"]), "1.2.3"),
        Invocation::Print("1.2.3\n".to_string())
    );
}
