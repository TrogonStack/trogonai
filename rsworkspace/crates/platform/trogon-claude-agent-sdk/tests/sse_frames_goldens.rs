//! Checks the SSE wire parser against what the browser transport's parser produced for the same chunks in Node
//! (`.config/mise/lib/claude-sdk-freshness/sse_frames_oracle.mjs` writes the goldens).

use serde_json::Value;
use trogon_claude_agent_sdk::{SseFrame, SseParser};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const PARSER: &str = include_str!("fixtures/sse_frames/parser.json");

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    value.get(key).ok_or_else(|| format!("missing {key} in {value}"))
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn frames(value: &Value) -> Result<Vec<SseFrame>, String> {
    value
        .as_array()
        .ok_or_else(|| format!("frames are not a list: {value}"))?
        .iter()
        .map(|frame| {
            Ok(SseFrame {
                event: text_field(frame, "event"),
                id: text_field(frame, "id"),
                data: text_field(frame, "data"),
            })
        })
        .collect()
}

fn chunk_bytes(chunk: &Value) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if let Some(text) = chunk.get("text").and_then(Value::as_str) {
        return Ok(text.as_bytes().to_vec());
    }
    let hex = field(chunk, "hex")?.as_str().ok_or("hex is not text")?;
    (0..hex.len())
        .step_by(2)
        .map(|at| Ok(u8::from_str_radix(&hex[at..at + 2], 16)?))
        .collect()
}

fn count(value: &Value, key: &str) -> Result<u64, String> {
    field(value, key)?
        .as_u64()
        .ok_or_else(|| format!("{key} is not a count"))
}

#[test]
fn every_chunking_dispatches_the_frames_the_browser_parser_dispatched() -> TestResult {
    let cases: Vec<Value> = serde_json::from_str(PARSER)?;
    assert!(cases.len() > 1000, "the corpus shrank to {} cases", cases.len());
    for case in &cases {
        let chunks = field(case, "chunks")?.as_array().ok_or("chunks are not a list")?;
        let steps = field(case, "steps")?.as_array().ok_or("steps are not a list")?;
        let mut parser = SseParser::new();
        for (chunk, step) in chunks.iter().zip(steps) {
            let got = parser.push(&chunk_bytes(chunk)?);
            assert_eq!(got, frames(field(step, "frames")?)?, "frames of {case}");
            assert_eq!(
                u64::try_from(parser.pending_chars())?,
                count(step, "pending_chars")?,
                "pending of {case}"
            );
            assert_eq!(
                parser.frames_dispatched(),
                count(step, "dispatched")?,
                "dispatched of {case}"
            );
        }
        assert_eq!(
            parser.buffered(),
            field(case, "buffered")?.as_str().ok_or("buffered is not text")?
        );
        assert_eq!(parser.flush(), frames(field(case, "flushed")?)?, "flush of {case}");
        assert_eq!(
            parser.frames_dispatched(),
            count(case, "dispatched")?,
            "total of {case}"
        );
    }
    Ok(())
}
