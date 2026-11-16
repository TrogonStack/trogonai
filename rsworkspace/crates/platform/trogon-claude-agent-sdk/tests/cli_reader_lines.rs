#![cfg(feature = "test-support")]

//! Replays `fixtures/cli_reader/lines.json`, recorded from the real Python and TypeScript SDKs, through the CLI
//! stdout reader: each line once followed by a newline and once as the unterminated tail of the stream.

use std::{error::Error as StdError, fmt::Write as _, time::Duration};

use futures_util::{StreamExt as _, stream};
use serde_json::{Map, Value};
use tokio::time::timeout;
use trogon_claude_agent_sdk::{
    ArgvStyle, ClaudeAgentOptions, ClaudeSdkClient, Error, Message, MessageDecoding, Prompt, Timeout,
};

type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

const LINES: &str = include_str!("fixtures/cli_reader/lines.json");

/// What a reader delivered or raised, in a form that compares numbers by kind and bits.
#[derive(Debug, PartialEq)]
enum Seen {
    Message(String),
    Raised(String, String),
}

fn canonical_into(value: &Value, typescript: bool, out: &mut String) {
    match value {
        Value::Number(number) if typescript || number.is_f64() => {
            let _ = write!(out, "f{:016x}", number.as_f64().unwrap_or(f64::NAN).to_bits());
        }
        Value::Number(number) => {
            let _ = write!(out, "i{number}");
        }
        Value::Array(items) => {
            out.push('[');
            for item in items {
                canonical_into(item, typescript, out);
                out.push(',');
            }
            out.push(']');
        }
        Value::Object(object) => canonical_object(object, typescript, out),
        other => out.push_str(&other.to_string()),
    }
}

fn canonical_object(object: &Map<String, Value>, typescript: bool, out: &mut String) {
    out.push('{');
    for (key, value) in object {
        out.push_str(&Value::String(key.clone()).to_string());
        out.push(':');
        canonical_into(value, typescript, out);
        out.push(',');
    }
    out.push('}');
}

/// Python reports a `UnicodeDecodeError` position relative to the chunk it read, which depends on pipe timing.
fn mask_position(text: &str) -> String {
    match text.split_once("position ") {
        Some((head, tail)) => {
            let rest = tail.trim_start_matches(|character: char| character.is_ascii_digit());
            format!("{head}position N{rest}")
        }
        None => text.to_owned(),
    }
}

fn expected_entry(entry: &Value, typescript: bool, out: &mut Vec<Seen>) -> TestResult {
    if let Some(text) = entry.get("message").and_then(Value::as_str) {
        let mut canonical = String::new();
        canonical_into(&serde_json::from_str(text)?, typescript, &mut canonical);
        out.push(Seen::Message(canonical));
    } else if let Some(class) = entry.get("raised").and_then(Value::as_str) {
        let text = entry.get("text").and_then(Value::as_str).unwrap_or_default();
        out.push(Seen::Raised(class.to_owned(), mask_position(text)));
    } else if let Some(rust) = entry.get("rust") {
        if !rust.is_null() {
            expected_entry(rust, typescript, out)?;
        }
    } else {
        return Err(std::io::Error::other(format!("unknown entry {entry}")).into());
    }
    Ok(())
}

fn raised(error: &Error) -> Seen {
    let class = match error {
        Error::JsonDecode(_) => "CLIJSONDecodeError",
        Error::Recursion(_) => "RecursionError",
        Error::IntDigits(_) => "ValueError",
        Error::UnicodeDecode(_) => "UnicodeDecodeError",
        Error::NullMessage(_) => "TypeError",
        other => return Seen::Raised(format!("{other:?}"), other.to_string()),
    };
    Seen::Raised(class.to_owned(), mask_position(&error.to_string()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

async fn replay(mode: &str, style: ArgvStyle, line_hex: &str) -> TestResult<Vec<Seen>> {
    let mut options = ClaudeAgentOptions {
        cli_path: Some(std::path::absolute(env!("CARGO_BIN_EXE_claude-sdk-fixture"))?),
        model: Some(format!("fixture:{mode}").into()),
        argv_style: style,
        ..ClaudeAgentOptions::default()
    };
    if style == ArgvStyle::Python {
        options.message_decoding = MessageDecoding::Python;
    }
    options.limits.control_timeout = Some(Timeout::new(Duration::from_secs(5)));
    options.limits.shutdown = trogon_claude_agent_sdk::Shutdown::Strict(Timeout::new(Duration::from_millis(500)));
    options.env.insert("LLVM_PROFILE_FILE".into(), "/dev/null".into());
    options.env.insert("FIXTURE_LINE_HEX".into(), line_hex.into());
    let typescript = style != ArgvStyle::Python;
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("go")).await?;
    client.finish_input().await?;
    let seen = timeout(Duration::from_secs(20), async {
        let mut seen = Vec::new();
        loop {
            match client.next_message().await {
                Ok(Some(Message::System(system))) if system.subtype != "init" => {
                    let mut canonical = String::new();
                    canonical_object(&system.raw, typescript, &mut canonical);
                    seen.push(Seen::Message(canonical));
                }
                Ok(Some(_)) => {}
                Ok(None) => return seen,
                Err(error) => {
                    seen.push(raised(&error));
                    return seen;
                }
            }
        }
    })
    .await?;
    let _ = client.disconnect().await;
    Ok(seen)
}

#[tokio::test]
async fn each_line_reads_as_the_upstream_reader_reads_it() -> TestResult {
    let fixture: Value = serde_json::from_str(LINES)?;
    let cases = fixture
        .get("cases")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(cases.len() > 100);
    let mut jobs = Vec::new();
    for case in &cases {
        let name = case.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
        let line_hex = match (
            case.get("line").and_then(Value::as_str),
            case.get("line_hex").and_then(Value::as_str),
        ) {
            (Some(line), _) => hex(line.as_bytes()),
            (None, Some(line_hex)) => line_hex.to_owned(),
            (None, None) => return Err(std::io::Error::other(format!("{name} has no line")).into()),
        };
        for (key, style, mode) in [
            ("python_line", ArgvStyle::Python, "line"),
            ("python_tail", ArgvStyle::Python, "line-tail"),
            ("typescript_line", ArgvStyle::TypeScript, "line"),
            ("typescript_tail", ArgvStyle::TypeScript, "line-tail"),
        ] {
            let mut expected = Vec::new();
            for entry in case.get(key).and_then(Value::as_array).cloned().unwrap_or_default() {
                expected_entry(&entry, style != ArgvStyle::Python, &mut expected)?;
            }
            if let Some(first) = expected.iter().position(|seen| matches!(seen, Seen::Raised(..))) {
                expected.truncate(first + 1);
            }
            jobs.push((format!("{name} {key}"), style, mode, line_hex.clone(), expected));
        }
    }
    let mismatches: Vec<String> = stream::iter(jobs)
        .map(|(label, style, mode, line_hex, mut expected)| async move {
            let mut actual = match replay(mode, style, &line_hex).await {
                Ok(actual) => actual,
                Err(error) => return Some(format!("{label}: {error}")),
            };
            // Python decodes whole pipe chunks, so which messages precede a UnicodeDecodeError depends on timing.
            if matches!(expected.last(), Some(Seen::Raised(class, _)) if class == "UnicodeDecodeError") {
                expected.drain(..expected.len() - 1);
                let keep = actual.len().saturating_sub(1);
                actual.drain(..keep);
            }
            (actual != expected).then(|| format!("{label}: expected {expected:?}, got {actual:?}"))
        })
        .buffer_unordered(8)
        .filter_map(|mismatch| async move { mismatch })
        .collect()
        .await;
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    Ok(())
}
