use std::io;

use serde_json::Value;

use super::{PATTERNS, redact};
use crate::{
    Error,
    error::{SpawnFailure, launch_failure},
    framing::SharedTail,
};

type TestResult = Result<(), String>;

const GOLDENS: &str = include_str!("../../tests/fixtures/stderr_redaction/goldens.json");

fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for character in text.bytes().take_while(|byte| *byte != b'=') {
        let value = match character {
            b'A'..=b'Z' => character - b'A',
            b'a'..=b'z' => character - b'a' + 26,
            b'0'..=b'9' => character - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(format!("not base64: {character}")),
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push(u8::try_from((buffer >> bits) & 0xff).map_err(|error| error.to_string())?);
        }
    }
    Ok(bytes)
}

fn decoded_text(value: &Value) -> Result<String, String> {
    let encoded = value.as_str().ok_or("a base64 string is missing")?;
    String::from_utf8(decode_base64(encoded)?).map_err(|error| error.to_string())
}

fn cases(name: &str) -> Result<Vec<Value>, String> {
    let goldens: Value = serde_json::from_str(GOLDENS).map_err(|error| error.to_string())?;
    goldens
        .get(name)
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| format!("{name} is missing"))
}

fn field<'a>(case: &'a Value, name: &str) -> Result<&'a Value, String> {
    case.get(name).ok_or_else(|| format!("{name} is missing"))
}

#[test]
fn base64_decoding_handles_padding_and_rejects_other_characters() -> TestResult {
    assert_eq!(decode_base64("aGk=")?, b"hi");
    assert_eq!(decode_base64("Pz8/Pg==")?, b"???>");
    assert!(decode_base64("a*").is_err());
    Ok(())
}

#[cfg(unix)]
fn failed_with_code_one(tail: &SharedTail) -> Error {
    use std::os::unix::process::ExitStatusExt;
    Error::Process {
        status: std::process::ExitStatus::from_raw(1 << 8),
        stderr: tail.snapshot(),
    }
}

#[cfg(unix)]
#[test]
fn every_golden_stderr_case_matches_the_bundle_byte_for_byte() -> TestResult {
    let stderr_cases = cases("stderr_cases")?;
    assert_eq!(stderr_cases.len(), 87);
    for case in &stderr_cases {
        let name = field(case, "name")?.as_str().unwrap_or_default();
        let tail = SharedTail::new(2048, true);
        for chunk in field(case, "chunks_b64")?.as_array().ok_or("chunks are missing")? {
            let encoded = chunk.as_str().ok_or("a chunk is missing")?;
            tail.append(&decode_base64(encoded)?);
        }
        tail.finish();
        let expected = decoded_text(field(case, "message_b64")?)?;
        let actual = failed_with_code_one(&tail).typescript_message();
        assert_eq!(actual.as_deref(), Some(expected.as_str()), "{name}");
    }
    Ok(())
}

#[test]
fn every_golden_spawn_case_matches_the_bundle_byte_for_byte() -> TestResult {
    let spawn_cases = cases("spawn_cases")?;
    assert_eq!(spawn_cases.len(), 7);
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let shown = directory.path().to_string_lossy().into_owned();
    for case in &spawn_cases {
        let name = field(case, "name")?.as_str().unwrap_or_default();
        let template = decoded_text(field(case, "path_template_b64")?)?;
        let expected = decoded_text(field(case, "message_b64")?)?.replace("{dir}", &shown);
        let path = std::path::PathBuf::from(template.replace("{dir}", &shown));
        if expected.contains("exists but failed") {
            let parent = path.parent().ok_or("a parent is missing")?;
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            std::fs::write(&path, b"#!/bin/sh\n").map_err(|error| error.to_string())?;
        }
        let missing = Error::CliNotFoundAt(path.clone());
        assert_eq!(
            missing.typescript_message().as_deref(),
            Some(expected.as_str()),
            "{name}"
        );
        let denied = Error::Spawn(launch_failure(&path, io::Error::from(io::ErrorKind::PermissionDenied)));
        assert_eq!(
            denied.typescript_message().as_deref(),
            Some(expected.as_str()),
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn a_launch_failure_keeps_its_kind_and_text_and_adds_the_path() -> TestResult {
    let original = io::Error::from(io::ErrorKind::PermissionDenied);
    let text = original.to_string();
    let wrapped = launch_failure(std::path::Path::new("/opt/claude"), original);
    assert_eq!(wrapped.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(wrapped.to_string(), text);
    let failure = wrapped
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<SpawnFailure>())
        .ok_or("the path was not kept")?;
    assert_eq!(failure.to_string(), text);
    Ok(())
}

#[cfg(unix)]
#[test]
fn the_errno_values_of_an_unusable_path_are_launch_failures() -> TestResult {
    for errno in [libc::ENOTDIR, libc::ELOOP, libc::ENAMETOOLONG, libc::EROFS] {
        let wrapped = launch_failure(std::path::Path::new("/opt/claude"), io::Error::from_raw_os_error(errno));
        let carries_path = wrapped
            .get_ref()
            .is_some_and(|inner| inner.downcast_ref::<SpawnFailure>().is_some());
        assert!(carries_path, "errno {errno}");
    }
    Ok(())
}

#[test]
fn any_other_spawn_error_is_left_as_it_was() {
    let wrapped = launch_failure(std::path::Path::new("/opt/claude"), io::Error::other("out of memory"));
    assert!(
        wrapped
            .get_ref()
            .is_some_and(|inner| inner.downcast_ref::<SpawnFailure>().is_none())
    );
    assert_eq!(wrapped.to_string(), "out of memory");
}

#[test]
fn another_spawn_error_is_worded_and_redacted_like_the_bundle() {
    let secret = format!("sk-{}", "Ab1".repeat(8));
    let error = Error::Spawn(io::Error::other(format!("cannot start {secret}")));
    assert_eq!(
        error.typescript_message().as_deref(),
        Some("Failed to spawn Claude Code process: cannot start <token>")
    );
}

fn token(prefix: &str, unit: &str, count: usize) -> String {
    format!("{prefix}{}", unit.repeat(count))
}

fn check(text: &str, expected: &str) {
    assert_eq!(redact(text), expected, "{text}");
}

#[test]
fn all_eighteen_patterns_are_compiled() {
    assert_eq!(PATTERNS.len(), 18);
    assert_eq!(super::COMPILED.len(), 18);
}

#[test]
fn a_bearer_token_needs_enough_characters() {
    check(
        &format!("Authorization: Bearer {}", "Ab1".repeat(8)),
        "Authorization: Bearer <token>",
    );
    check("Authorization: bearer abc", "Authorization: bearer abc");
    check(
        &format!("bearer {}%2F{}", "Ab1".repeat(3), "Cd2".repeat(3)),
        "Bearer <token>",
    );
}

#[test]
fn a_basic_credential_follows_a_colon() {
    check(
        &format!("authorization: Basic {}", "QWxh".repeat(3)),
        "authorization: Basic <token>",
    );
    check(
        &format!("Basic {}", "QWxh".repeat(3)),
        &format!("Basic {}", "QWxh".repeat(3)),
    );
}

#[test]
fn the_anthropic_key_forms_are_removed() {
    check(&format!("key {}", token("sk-ant-", "Ab1", 4)), "key <token>");
    check(&format!("key {}", token("sk-ant-", "api03-", 3)), "key <token>");
    check("sk-ant-short", "sk-ant-short");
}

#[test]
fn the_openai_key_forms_are_removed() {
    check(&format!("key {}", token("sk-proj-", "Ab1", 8)), "key <token>");
    check(&format!("key {}", token("sk-svcacct-", "Ab1", 8)), "key <token>");
    check(&format!("key {}", token("sk-", "Ab1", 8)), "key <token>");
    check("sk-short", "sk-short");
}

#[test]
fn an_aws_access_key_keeps_the_character_before_it() {
    check(&format!("id AKIA{} end", "AB1C".repeat(4)), "id <token> end");
    check(&format!("AKIA{}", "AB1C".repeat(4)), "<token>");
    check(&format!("-AKIA{}", "AB1C".repeat(4)), "-<token>");
    check(
        &format!("xAKIA{}", "AB1C".repeat(4)),
        &format!("xAKIA{}", "AB1C".repeat(4)),
    );
    check(
        &format!("AKIA{}x", "AB1C".repeat(4)),
        &format!("AKIA{}x", "AB1C".repeat(4)),
    );
}

#[test]
fn a_github_token_is_removed() {
    check(&format!("t {}", token("ghp_", "Ab1", 8)), "t <token>");
    check(&format!("t {}", token("ghs_", "Ab1", 8)), "t <token>");
    check(&format!("t {}", token("github_pat_", "Ab1", 28)), "t <token>");
}

#[test]
fn a_square_token_is_removed() {
    check(&format!("t {}", token("sq0atp-", "Ab1", 8)), "t <token>");
    check(&format!("t {}", token("sq0csp-", "Ab1", 8)), "t <token>");
}

#[test]
fn a_square_access_token_keeps_the_character_before_it() {
    check(&format!("t {}", token("EAAA", "Ab1", 20)), "t <token>");
}

#[test]
fn a_gitlab_token_is_removed() {
    check(&format!("t {}", token("glpat-", "Ab1", 8)), "t <token>");
    check(&format!("t {}", token("gldt-", "Ab1", 8)), "t <token>");
}

#[test]
fn a_slack_token_is_removed_in_either_case() {
    check(&format!("t {}", token("xoxb-", "12ab", 4)), "t <token>");
    check(&format!("t {}", token("XOXB-", "12AB", 4)), "t <token>");
    check(&format!("t {}", token("xapp-", "12ab", 4)), "t <token>");
    check(&format!("t {}", token("xoxc-", "12ab", 4)), "t <token>");
    check(&format!("t {}", token("xoxd-", "12ab", 4)), "t <token>");
}

#[test]
fn a_slack_webhook_is_shortened() {
    check(
        "post hooks.slack.com/services/T000/B000/abcdef now",
        "post hooks.slack.com/<redacted> now",
    );
    check("HOOKS.SLACK.COM/workflows/A1/b2", "hooks.slack.com/<redacted>");
}

#[test]
fn a_jwt_is_removed_and_a_prefix_before_its_header_is_kept() {
    let segment = "AbCdEfGhIj";
    check(&format!("t ey{segment}.{segment}.{segment} end"), "t <jwt> end");
    check(&format!("t abc-ey{segment}.{segment}.{segment}"), "t abc-<jwt>");
    check("eyshort.eyshort.eyshort", "eyshort.eyshort.eyshort");
}

#[test]
fn a_google_key_keeps_the_character_before_it() {
    check(&format!("key AIza{}{} end", "Ab1".repeat(11), "Ab"), "key <token> end");
    check(
        &format!("AIza{}", "Ab1".repeat(11)),
        &format!("AIza{}", "Ab1".repeat(11)),
    );
}

#[test]
fn a_google_client_secret_keeps_the_character_before_it() {
    check(&format!("s GOCSPX-{}A end", "Ab1".repeat(9)), "s <token> end");
}

#[test]
fn a_stripe_key_keeps_the_character_before_it() {
    check(&format!("s {}", token("sk_live_", "Ab1", 8)), "s <token>");
    check(&format!("s {}", token("rk_test_", "Ab1", 8)), "s <token>");
}

#[test]
fn text_without_a_secret_is_unchanged() {
    check("plain stderr: nothing to see", "plain stderr: nothing to see");
    check("", "");
}
