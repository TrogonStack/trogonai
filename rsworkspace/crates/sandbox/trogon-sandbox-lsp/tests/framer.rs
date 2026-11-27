use serde_json::{Value, json};
use trogon_sandbox_lsp::{LspError, LspFramer, encode_message};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn frame(body: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

#[test]
fn encodes_byte_length_not_char_length() -> TestResult {
    let encoded = encode_message(&json!({ "t": "héllo 世界" }))?;
    let text = String::from_utf8(encoded)?;
    let (header, body) = text.split_once("\r\n\r\n").ok_or("no header")?;
    assert_eq!(header, format!("Content-Length: {}", body.len()));
    assert!(body.len() > body.chars().count());
    Ok(())
}

#[test]
fn encoding_a_message_that_is_not_json_fails() {
    let unserializable: std::collections::HashMap<Vec<u8>, u8> = [(vec![1], 1)].into();
    assert!(matches!(encode_message(&unserializable), Err(LspError::Protocol(_))));
}

#[test]
fn handles_headers_split_across_chunks() -> TestResult {
    let bytes = frame(r#"{"a":1}"#);
    let mut framer = LspFramer::new();
    assert!(framer.push(&bytes[..5])?.is_empty());
    assert!(framer.push(&bytes[5..19])?.is_empty());
    assert_eq!(framer.push(&bytes[19..])?, vec![r#"{"a":1}"#.to_string()]);
    Ok(())
}

#[test]
fn handles_terminator_split_in_the_middle() -> TestResult {
    let bytes = frame("{}");
    let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no terminator")? + 2;
    let mut framer = LspFramer::new();
    assert!(framer.push(&bytes[..split])?.is_empty());
    assert_eq!(framer.push(&bytes[split..])?, vec!["{}".to_string()]);
    Ok(())
}

#[test]
fn splits_coalesced_messages() -> TestResult {
    let mut bytes = frame(r#"{"a":1}"#);
    bytes.extend(frame(r#"{"b":2}"#));
    bytes.extend(frame(r#"{"c":3}"#));
    assert_eq!(
        LspFramer::new().push(&bytes)?,
        vec![r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":3}"#]
    );
    Ok(())
}

#[test]
fn keeps_partial_trailing_message_for_the_next_chunk() -> TestResult {
    let mut bytes = frame(r#"{"a":1}"#);
    let second = frame(r#"{"b":2}"#);
    bytes.extend_from_slice(&second[..second.len() - 3]);
    let mut framer = LspFramer::new();
    assert_eq!(framer.push(&bytes)?, vec![r#"{"a":1}"#]);
    assert_eq!(framer.push(&second[second.len() - 3..])?, vec![r#"{"b":2}"#]);
    Ok(())
}

#[test]
fn counts_multibyte_utf8_by_bytes_even_when_split_inside_a_character() -> TestResult {
    let body = r#"{"t":"héllo 世界 😀"}"#;
    let bytes = frame(body);
    let start = bytes.len() - body.len();
    let emoji_at = start + body.find('😀').ok_or("no emoji")?;
    let mut framer = LspFramer::new();
    assert!(framer.push(&bytes[..emoji_at + 2])?.is_empty());
    assert_eq!(framer.push(&bytes[emoji_at + 2..])?, vec![body.to_string()]);
    Ok(())
}

#[test]
fn accepts_extra_headers_and_case_insensitive_names() -> TestResult {
    let body = "{}";
    let bytes = format!(
        "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    assert_eq!(LspFramer::new().push(bytes.as_bytes())?, vec![body.to_string()]);
    Ok(())
}

#[test]
fn rejects_missing_or_invalid_content_length() {
    assert!(matches!(
        LspFramer::new().push(b"not a header line\r\nX-Other: 1\r\n\r\n"),
        Err(LspError::Protocol(m)) if m.contains("missing Content-Length")
    ));
    assert!(matches!(
        LspFramer::new().push(b"Content-Length: abc\r\n\r\n"),
        Err(LspError::Protocol(m)) if m.contains("invalid Content-Length")
    ));
    assert!(matches!(
        LspFramer::new().push(b"Content-Length: 999999999999\r\n\r\n"),
        Err(LspError::Protocol(m)) if m.contains("exceeds limit")
    ));
}

#[test]
fn rejects_unbounded_header_and_invalid_utf8_body() {
    assert!(matches!(
        LspFramer::new().push(&vec![b'a'; 8 * 1024 + 1]),
        Err(LspError::Protocol(m)) if m.contains("header section")
    ));
    let mut bytes = b"Content-Length: 2\r\n\r\n".to_vec();
    bytes.extend([0xff, 0xfe]);
    assert!(matches!(
        LspFramer::new().push(&bytes),
        Err(LspError::Protocol(m)) if m.contains("UTF-8")
    ));
}

#[test]
fn decoded_bodies_parse_as_json() -> TestResult {
    let bodies = LspFramer::new().push(&encode_message(&json!({ "k": 1 }))?)?;
    assert_eq!(serde_json::from_str::<Value>(&bodies[0])?, json!({ "k": 1 }));
    Ok(())
}
