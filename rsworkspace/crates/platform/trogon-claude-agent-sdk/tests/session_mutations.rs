#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Fixture transcripts are exact upstream-shaped JSONL lines."
    )
)]

use std::{error::Error, fs, path::PathBuf};

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{ListSessionsOptions, MessagePage, SessionFiles, SessionMutationError};

type TestResult = Result<(), Box<dyn Error>>;

const SESSION: &str = "123e4567-e89b-42d3-a456-426614174000";
const U1: &str = "523e4567-e89b-42d3-a456-426614174001";
const A1: &str = "523e4567-e89b-42d3-a456-426614174002";
const P1: &str = "523e4567-e89b-42d3-a456-426614174003";
const U2: &str = "523e4567-e89b-42d3-a456-426614174004";

struct Fixture {
    _config: tempfile::TempDir,
    files: SessionFiles,
    dir: PathBuf,
}

fn fixture(lines: &[Value]) -> Result<Fixture, Box<dyn Error>> {
    let config = tempfile::tempdir()?;
    let dir = config.path().join("projects").join("-proj");
    fs::create_dir_all(&dir)?;
    let body: String = lines.iter().map(|line| format!("{line}\n")).collect();
    fs::write(dir.join(format!("{SESSION}.jsonl")), body)?;
    let files = SessionFiles::with_config_dir(config.path());
    Ok(Fixture {
        _config: config,
        files,
        dir,
    })
}

fn transcript() -> Vec<Value> {
    vec![
        json!({"type":"user","uuid":U1,"parentUuid":null,"sessionId":SESSION,"timestamp":"2024-01-01T00:00:00.000Z",
               "teamName":"t","slug":"s","message":{"role":"user","content":"hello world"}}),
        json!({"type":"progress","uuid":P1,"parentUuid":U1,"sessionId":SESSION}),
        json!({"type":"assistant","uuid":A1,"parentUuid":P1,"sessionId":SESSION,"timestamp":"2024-01-01T00:00:01.000Z",
               "message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"hi"}]}}),
        json!({"type":"content-replacement","sessionId":SESSION,"replacements":[{"id":"r1"}]}),
        json!({"type":"user","uuid":U2,"parentUuid":A1,"sessionId":SESSION,"timestamp":"2024-01-01T00:00:02.000Z",
               "isSidechain":false,"message":{"role":"user","content":"again"}}),
    ]
}

#[test]
fn rename_appends_a_title_that_listing_reads_back() -> TestResult {
    let f = fixture(&transcript())?;
    f.files.rename_session(SESSION, "  My title  ", None)?;
    let body = fs::read_to_string(f.dir.join(format!("{SESSION}.jsonl")))?;
    let last = body.lines().last().ok_or("empty")?;
    assert_eq!(
        last,
        format!("{{\"type\":\"custom-title\",\"customTitle\":\"My title\",\"sessionId\":\"{SESSION}\"}}")
    );
    let info = f.files.get_session_info(SESSION, None).ok_or("missing")?;
    assert_eq!(info.custom_title.as_deref(), Some("My title"));
    assert!(matches!(
        f.files.rename_session(SESSION, " \t", None),
        Err(SessionMutationError::EmptyTitle)
    ));
    assert!(matches!(
        f.files.rename_session("nope", "x", None),
        Err(SessionMutationError::InvalidSessionId(_))
    ));
    assert!(matches!(
        f.files
            .rename_session("223e4567-e89b-42d3-a456-426614174009", "x", None),
        Err(SessionMutationError::NotFound { .. })
    ));
    Ok(())
}

#[test]
fn tag_sanitizes_unicode_and_none_clears() -> TestResult {
    let f = fixture(&transcript())?;
    f.files
        .tag_session(SESSION, Some("\u{200b}exp\u{feff}\u{e000}eriment \u{202e}"), None)?;
    let info = f.files.get_session_info(SESSION, None).ok_or("missing")?;
    assert_eq!(info.tag.as_deref(), Some("experiment"));
    f.files.tag_session(SESSION, Some("\u{ff21}"), None)?;
    let info = f.files.get_session_info(SESSION, None).ok_or("missing")?;
    assert_eq!(info.tag.as_deref(), Some("A"), "NFKC folds full-width letters");
    f.files.tag_session(SESSION, None, None)?;
    let info = f.files.get_session_info(SESSION, None).ok_or("missing")?;
    assert_eq!(info.tag, None);
    assert!(matches!(
        f.files.tag_session(SESSION, Some("\u{200b}\u{200d}"), None),
        Err(SessionMutationError::EmptyTag)
    ));
    let body = fs::read_to_string(f.dir.join(format!("{SESSION}.jsonl")))?;
    assert!(body.contains(&format!(
        "{{\"type\":\"tag\",\"tag\":\"\",\"sessionId\":\"{SESSION}\"}}"
    )));
    Ok(())
}

#[test]
fn append_skips_empty_stubs_and_searches_every_project() -> TestResult {
    let f = fixture(&transcript())?;
    let stub_dir = f.dir.parent().ok_or("parent")?.join("-aaa-stub");
    fs::create_dir_all(&stub_dir)?;
    fs::write(stub_dir.join(format!("{SESSION}.jsonl")), "")?;
    f.files.tag_session(SESSION, Some("keep"), None)?;
    assert_eq!(fs::metadata(stub_dir.join(format!("{SESSION}.jsonl")))?.len(), 0);
    let info = f.files.get_session_info(SESSION, None).ok_or("missing")?;
    assert_eq!(info.tag.as_deref(), Some("keep"));
    Ok(())
}

#[test]
fn delete_removes_the_transcript_and_subagent_directory() -> TestResult {
    let f = fixture(&transcript())?;
    let subagents = f.dir.join(SESSION).join("subagents");
    fs::create_dir_all(&subagents)?;
    fs::write(subagents.join("agent-a.jsonl"), "{}")?;
    f.files.delete_session(SESSION, None)?;
    assert!(!f.dir.join(format!("{SESSION}.jsonl")).exists());
    assert!(!f.dir.join(SESSION).exists());
    assert!(matches!(
        f.files.delete_session(SESSION, None),
        Err(SessionMutationError::NotFound { .. })
    ));
    Ok(())
}

#[test]
fn fork_remaps_uuids_skips_progress_and_titles_the_copy() -> TestResult {
    let f = fixture(&transcript())?;
    let forked = f.files.fork_session(SESSION, None, None, None)?.session_id;
    let path = f.dir.join(format!("{}.jsonl", forked.as_str()));
    let lines: Vec<Value> = fs::read_to_string(&path)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    // user, assistant, user, content-replacement, custom-title (progress dropped)
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0]["type"], "user");
    assert_eq!(lines[0]["sessionId"], forked.as_str());
    assert_ne!(lines[0]["uuid"], U1);
    assert_eq!(lines[0]["forkedFrom"], json!({"sessionId":SESSION,"messageUuid":U1}));
    assert_eq!(lines[0]["isSidechain"], false);
    assert_eq!(lines[0]["parentUuid"], Value::Null);
    assert_eq!(lines[0]["logicalParentUuid"], Value::Null);
    assert!(lines[0].get("teamName").is_none() && lines[0].get("slug").is_none());
    assert_eq!(
        lines[1]["parentUuid"], lines[0]["uuid"],
        "progress ancestors are skipped"
    );
    assert_eq!(lines[2]["parentUuid"], lines[1]["uuid"]);
    assert_eq!(lines[0]["timestamp"], "2024-01-01T00:00:00.000Z");
    assert_ne!(
        lines[2]["timestamp"], "2024-01-01T00:00:02.000Z",
        "the leaf is restamped"
    );
    assert_eq!(lines[3]["type"], "content-replacement");
    assert_eq!(lines[3]["replacements"], json!([{"id":"r1"}]));
    assert_eq!(lines[4]["type"], "custom-title");
    assert_eq!(lines[4]["customTitle"], "hello world (fork)");

    let messages = f
        .files
        .get_session_messages(forked.as_str(), None, MessagePage::default())?;
    assert_eq!(messages.len(), 3);
    let listed = f.files.list_sessions(&ListSessionsOptions::default());
    assert_eq!(listed.len(), 2);
    Ok(())
}

#[test]
fn fork_honors_the_cutoff_and_an_explicit_title() -> TestResult {
    let f = fixture(&transcript())?;
    let forked = f
        .files
        .fork_session(SESSION, None, Some(A1), Some("  Branch  "))?
        .session_id;
    let body = fs::read_to_string(f.dir.join(format!("{}.jsonl", forked.as_str())))?;
    let lines: Vec<Value> = body.lines().map(serde_json::from_str).collect::<Result<_, _>>()?;
    assert_eq!(lines.len(), 4, "two messages, the replacement, and the title");
    assert_eq!(lines[3]["customTitle"], "Branch");
    assert!(matches!(
        f.files
            .fork_session(SESSION, None, Some("223e4567-e89b-42d3-a456-426614174009"), None),
        Err(SessionMutationError::MessageNotFound { .. })
    ));
    assert!(matches!(
        f.files.fork_session(SESSION, None, Some("bad"), None),
        Err(SessionMutationError::InvalidMessageId(_))
    ));
    Ok(())
}

#[test]
fn fork_derives_titles_from_existing_custom_titles_and_rejects_empty_sources() -> TestResult {
    let mut lines = transcript();
    lines.push(json!({"type":"custom-title","customTitle":"Original","sessionId":SESSION}));
    let f = fixture(&lines)?;
    let forked = f.files.fork_session(SESSION, None, None, None)?.session_id;
    let body = fs::read_to_string(f.dir.join(format!("{}.jsonl", forked.as_str())))?;
    assert!(body.lines().last().ok_or("empty")?.contains("Original (fork)"));

    let sidechain_only = fixture(&[json!({"type":"user","uuid":U1,"isSidechain":true})])?;
    assert!(matches!(
        sidechain_only.files.fork_session(SESSION, None, None, None),
        Err(SessionMutationError::NoMessages(_))
    ));
    Ok(())
}

#[test]
fn fork_keeps_source_key_order_and_selects_the_escaping() -> TestResult {
    let mut lines = transcript();
    lines[0] = json!({"type":"user","uuid":U1,"parentUuid":null,"sessionId":SESSION,"slug":"s","zebra":1,
                      "alpha":2,"teamName":"t","message":{"role":"user","content":"caf\u{e9} \u{1f600}"}});
    let f = fixture(&lines)?;

    let utf8 = f.files.fork_session(SESSION, None, None, None)?.session_id;
    let body = fs::read_to_string(f.dir.join(format!("{}.jsonl", utf8.as_str())))?;
    let first = body.lines().next().ok_or("empty")?;
    assert!(first.contains("caf\u{e9} \u{1f600}"));
    let zebra = first.find("\"zebra\"").ok_or("zebra")?;
    let alpha = first.find("\"alpha\"").ok_or("alpha")?;
    assert!(first.find("\"type\"").ok_or("type")? < zebra && zebra < alpha);
    assert!(!first.contains("teamName") && !first.contains("\"slug\""));

    let ascii = f
        .files
        .fork_session_with(SESSION, None, None, None, trogon_claude_agent_sdk::JsonEscaping::Ascii)?
        .session_id;
    let body = fs::read_to_string(f.dir.join(format!("{}.jsonl", ascii.as_str())))?;
    let first = body.lines().next().ok_or("empty")?;
    assert!(first.contains("caf\\u00e9 \\ud83d\\ude00"));
    assert!(first.is_ascii());
    Ok(())
}
