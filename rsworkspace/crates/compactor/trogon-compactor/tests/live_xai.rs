//! Live test: the trogon-compactor summarizes a real conversation via the REAL
//! xAI API (OpenAI-compatible Chat Completions). This is the "live" half of
//! Gap B — context compaction in xai-runner: the runner sends history to the
//! compactor, and the compactor produces a structured checkpoint via the
//! session's provider (here, xAI).
//!
//! `#[ignore]` by default — never runs in CI. Requires `XAI_API_KEY`.
//! Run with:
//!   cargo test -p trogon-compactor --test live_xai -- --ignored --nocapture

use trogon_compactor::{AuthStyle, LlmConfig, LlmProvider, Message, OpenAICompatLlmProvider};

#[ignore = "requires XAI_API_KEY — hits the real xAI API"]
#[tokio::test]
async fn compactor_summarizes_conversation_via_real_xai() {
    let api_key = std::env::var("XAI_API_KEY").expect("XAI_API_KEY must be set for the live test");

    // xAI is OpenAI-compatible Chat Completions, Bearer auth — same path the
    // compactor service builds for provider "xai".
    let config = LlmConfig {
        api_url: "https://api.x.ai/v1/chat/completions".to_string(),
        api_key,
        auth_style: AuthStyle::Bearer,
        model: "grok-3".to_string(),
        max_summary_tokens: 512,
    };
    let provider = OpenAICompatLlmProvider::new(config);

    // A short coding conversation to compact.
    let messages = vec![
        Message::user("I'm building a Rust CLI that parses CSV files. Start with clap argument parsing."),
        Message::assistant("Added clap with `--input <path>` and `--delimiter` (default ','). Parses args in main()."),
        Message::user("Now add a function to count the number of rows using the csv crate."),
        Message::assistant("Added `count_rows(path) -> usize` using csv::Reader; wired into main after arg parsing."),
        Message::user("Bug: empty files panic. Fix count_rows to return 0 for empty files."),
    ];

    let summary = provider
        .generate_summary(&messages, None)
        .await
        .expect("xAI must return a summary");

    eprintln!("\n=== xAI-GENERATED SUMMARY ===\n{summary}\n=== END SUMMARY ===\n");

    assert!(!summary.is_empty(), "summary must not be empty");
    assert!(
        summary.contains("##"),
        "summary should follow the structured checkpoint template (## sections); got: {summary}"
    );
}
