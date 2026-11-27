use std::process::Command;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn through_sh(value: &str) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("printf %s {}", quote(value)))
        .output()?;
    Ok(String::from_utf8(output.stdout)?)
}

#[test]
fn a_word_survives_the_shell_unchanged() -> TestResult {
    for value in [
        "plain",
        "",
        "two words",
        "it's",
        "'''",
        "$HOME `id` $(id) ; & | > < * ?",
        "line\nbreak",
        "back\\slash \"double\"",
    ] {
        assert_eq!(through_sh(value)?, value);
    }
    Ok(())
}
