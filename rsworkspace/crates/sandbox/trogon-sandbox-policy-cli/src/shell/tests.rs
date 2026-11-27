#[cfg(unix)]
use trogon_std::env::{ReadEnv, SystemEnv};

use super::*;

#[cfg(unix)]
type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn plain_words_stay_as_they_are() {
    assert_eq!(quote("echo"), "echo");
    assert_eq!(quote("/usr/bin/env"), "/usr/bin/env");
}

#[test]
fn everything_else_is_single_quoted() {
    assert_eq!(quote(""), "''");
    assert_eq!(quote("hello world"), "'hello world'");
    assert_eq!(quote("$HOME;|&"), "'$HOME;|&'");
    assert_eq!(quote("it's"), "'it'\"'\"'s'");
    assert_eq!(quote("%s\n"), "'%s\n'");
    assert_eq!(quote("100%"), "'100%'");
    assert_eq!(quote("=ls"), "'=ls'");
    assert_eq!(quote("NAME=value"), "NAME=value");
    assert_eq!(quote("-n"), "-n");
}

#[test]
fn arguments_are_joined_by_spaces() {
    let args = vec!["printf".to_string(), "%s".to_string(), "a b".to_string()];
    assert_eq!(join(&args), "printf '%s' 'a b'");
}

#[test]
fn bare_words_and_single_quotes_are_kept_apart() {
    let args: Vec<String> = ["env", "A=b", "/usr/bin/sandbox-exec", "-p"].map(String::from).into();
    assert_eq!(join(&args), "env A=b /usr/bin/sandbox-exec -p");
    assert_eq!(quote("don't"), "'don'\"'\"'t'");
    assert_eq!(quote("a b"), "'a b'");
}

#[cfg(unix)]
fn round_trip(shell: &std::path::Path, args: &[&str]) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut words = vec!["printf".to_string(), "%s\\0".to_string()];
    words.extend(args.iter().map(|arg| arg.to_string()));
    let output = std::process::Command::new(shell).arg("-c").arg(join(&words)).output()?;
    assert!(output.status.success(), "{shell:?}");
    let text = String::from_utf8(output.stdout)?;
    let mut parts: Vec<String> = text.split('\0').map(String::from).collect();
    parts.pop();
    Ok(parts)
}

#[cfg(unix)]
#[test]
fn every_shell_on_the_machine_gets_the_arguments_back_unchanged() -> TestResult {
    let path = SystemEnv.var_os("PATH");
    let metacharacters = [
        "if (!ok) { throw new Error(\"bad\") }",
        "a != b",
        "!seen[$0]++",
        "don't",
        "$HOME `id` $(id)",
        "* ? [a-z] ~ # | & ; < > ( )",
        "back\\slash",
        "new\nline",
        "tab\there",
        "!",
        "",
        "%s",
        "100%",
        "%1",
        "=ls",
        "=definitely-not-a-command",
        "==",
        "=",
        "A=b",
    ];
    for name in ["bash", "zsh", "sh", "dash"] {
        let Some(shell) = find_in_path(name, path.as_deref()) else {
            continue;
        };
        assert_eq!(round_trip(&shell, &metacharacters)?, metacharacters, "{name}");
    }
    Ok(())
}

#[cfg(unix)]
fn shells() -> Vec<std::path::PathBuf> {
    ["bash", "zsh", "sh", "dash"]
        .into_iter()
        .filter_map(|name| find_in_path(name, SystemEnv.var_os("PATH").as_deref()))
        .collect()
}

#[cfg(unix)]
#[test]
fn any_printable_argv_round_trips_exactly() -> TestResult {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for shell in shells() {
        for _ in 0..25 {
            let words = 1 + next() % 6;
            let args: Vec<String> = (0..words)
                .map(|_| {
                    (0..next() % 12)
                        .map(|_| char::from(0x20 + (next() % 0x5f) as u8))
                        .collect()
                })
                .collect();
            let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
            assert_eq!(round_trip(&shell, &borrowed)?, args, "{shell:?}");
        }
    }
    Ok(())
}
