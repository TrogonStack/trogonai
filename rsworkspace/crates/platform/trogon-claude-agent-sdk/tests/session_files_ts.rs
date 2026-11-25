#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Fixture transcripts are exact upstream-shaped JSONL lines."
    )
)]

use std::{error::Error, fs, path::Path};

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{ListSessionsOptions, SessionFiles, SummaryDialect, project_key_for_directory_in};

type TestResult = Result<(), Box<dyn Error>>;

const SESSION: &str = "123e4567-e89b-42d3-a456-426614174000";
const OTHER: &str = "223e4567-e89b-42d3-a456-426614174001";
const THIRD: &str = "323e4567-e89b-42d3-a456-426614174002";

fn jsonl(lines: &[Value]) -> String {
    lines.iter().map(|line| line.to_string() + "\n").collect()
}

fn project(config: &Path, name: &str) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let dir = config.join("projects").join(name);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn prompt(text: &str) -> Value {
    json!({"type":"user","uuid":"u1","parentUuid":null,"timestamp":"2024-01-01T00:00:00.000Z",
           "message":{"role":"user","content":text}})
}

fn write(dir: &Path, id: &str, lines: &[Value]) -> TestResult {
    fs::write(dir.join(format!("{id}.jsonl")), jsonl(lines))?;
    Ok(())
}

fn only_info(files: &SessionFiles, id: &str) -> Result<trogon_claude_agent_sdk::SdkSessionInfo, Box<dyn Error>> {
    files
        .get_session_info(id, None)
        .ok_or_else(|| "session info missing".into())
}

#[test]
fn ts_title_precedence_is_tail_custom_then_sidecar_then_head_then_ai() -> TestResult {
    let config = tempfile::tempdir()?;
    let dir = project(config.path(), "-proj")?;
    write(
        &dir,
        SESSION,
        &[prompt("hello"), json!({"type":"ai-title","aiTitle":"Generated"})],
    )?;
    let files = SessionFiles::with_config_dir(config.path());
    assert_eq!(only_info(&files, SESSION)?.summary, "Generated");

    let sidecar = dir.join(SESSION);
    fs::create_dir_all(&sidecar)?;
    fs::write(
        sidecar.join("custom-title.json"),
        json!({"customTitle": "  From\u{200b}sidecar\n title  "}).to_string(),
    )?;
    let info = only_info(&files, SESSION)?;
    assert_eq!(info.custom_title.as_deref(), Some("From sidecar  title"));

    write(
        &dir,
        SESSION,
        &[
            prompt("hello"),
            json!({"type":"custom-title","customTitle":"Tail wins"}),
        ],
    )?;
    assert_eq!(only_info(&files, SESSION)?.custom_title.as_deref(), Some("Tail wins"));
    Ok(())
}

#[test]
fn ts_reads_the_newest_whole_line_not_a_matching_substring() -> TestResult {
    let config = tempfile::tempdir()?;
    let dir = project(config.path(), "-proj")?;
    // The newest custom-title line is garbled, so the older intact one is used.
    let body = format!(
        "{}\n{}\n{}\n{}\n",
        prompt("hello"),
        json!({"type":"custom-title","customTitle":"Older"}),
        json!({"type":"user","uuid":"x","message":{"content":"mentions \"customTitle\":\"Fake\""}}),
        "{\"type\":\"custom-title\",\"customTitle\":\"Broken"
    );
    fs::write(dir.join(format!("{SESSION}.jsonl")), body)?;
    let files = SessionFiles::with_config_dir(config.path());
    assert_eq!(only_info(&files, SESSION)?.custom_title.as_deref(), Some("Older"));
    Ok(())
}

#[test]
fn ts_relocated_cwd_and_media_only_summary() -> TestResult {
    let config = tempfile::tempdir()?;
    let dir = project(config.path(), "-proj")?;
    write(
        &dir,
        SESSION,
        &[
            json!({"type":"user","uuid":"u","cwd":"/old","timestamp":"2024-01-01T00:00:00.000Z",
                   "message":{"content":[{"type":"image","source":{}}]}}),
            json!({"type":"relocated","relocatedCwd":"/new"}),
        ],
    )?;
    let files = SessionFiles::with_config_dir(config.path());
    let info = only_info(&files, SESSION)?;
    assert_eq!(info.cwd.as_deref(), Some("/new"));
    assert_eq!(info.summary, "Image");
    assert_eq!(info.first_prompt, None);
    Ok(())
}

#[test]
fn programmatic_sessions_can_be_excluded() -> TestResult {
    let config = tempfile::tempdir()?;
    let dir = project(config.path(), "-proj")?;
    write(
        &dir,
        SESSION,
        &[json!({"type":"user","uuid":"u","parentUuid":null,"entrypoint":"sdk-ts",
                 "message":{"content":"sdk"}})],
    )?;
    write(
        &dir,
        OTHER,
        &[
            json!({"type":"user","uuid":"u","parentUuid":null,"sessionKind":"daemon",
                 "message":{"content":"daemon"}}),
        ],
    )?;
    write(
        &dir,
        THIRD,
        &[json!({"type":"user","uuid":"u","parentUuid":null,"entrypoint":"cli",
                 "message":{"content":"person"}})],
    )?;
    let files = SessionFiles::with_config_dir(config.path());
    assert_eq!(files.list_sessions(&ListSessionsOptions::default()).len(), 3);
    let filtered = files.list_sessions(&ListSessionsOptions {
        include_programmatic: false,
        ..ListSessionsOptions::default()
    });
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].session_id.as_str(), THIRD);
    Ok(())
}

#[test]
fn a_continued_session_is_hidden_once_its_continuation_has_a_conversation() -> TestResult {
    let config = tempfile::tempdir()?;
    let dir = project(config.path(), "-proj")?;
    write(
        &dir,
        SESSION,
        &[
            prompt("start"),
            json!({"type":"continued-in","continuedInSessionId":OTHER}),
        ],
    )?;
    let files = SessionFiles::with_config_dir(config.path());
    // The target does not exist yet, so the original stays visible.
    assert_eq!(files.list_sessions(&ListSessionsOptions::default()).len(), 1);
    write(&dir, OTHER, &[prompt("carry on")])?;
    let listed = files.list_sessions(&ListSessionsOptions::default());
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].session_id.as_str(), OTHER);
    // getSessionInfo applies no continued-in filtering.
    assert_eq!(only_info(&files, SESSION)?.summary, "start");

    // A real assistant turn after the marker means the session carried on.
    write(
        &dir,
        SESSION,
        &[
            prompt("start"),
            json!({"type":"continued-in","continuedInSessionId":OTHER}),
            json!({"type":"assistant","uuid":"a","message":{"stop_reason":"end_turn","content":[]}}),
        ],
    )?;
    assert_eq!(files.list_sessions(&ListSessionsOptions::default()).len(), 2);
    Ok(())
}

#[test]
fn paging_orders_by_file_time_and_skips_rejected_sessions() -> TestResult {
    let config = tempfile::tempdir()?;
    let dir = project(config.path(), "-proj")?;
    for (id, text) in [(SESSION, "one"), (OTHER, "two"), (THIRD, "three")] {
        write(&dir, id, &[prompt(text)])?;
    }
    let files = SessionFiles::with_config_dir(config.path());
    let all = files.list_sessions(&ListSessionsOptions::default());
    assert_eq!(all.len(), 3);
    let page = files.list_sessions(&ListSessionsOptions {
        limit: Some(2),
        offset: 1,
        ..ListSessionsOptions::default()
    });
    assert_eq!(page.len(), 2);
    let paged_ids: Vec<&str> = page.iter().map(|info| info.session_id.as_str()).collect();
    let all_ids: Vec<&str> = all.iter().map(|info| info.session_id.as_str()).collect();
    assert_eq!(paged_ids, all_ids[1..3]);
    Ok(())
}

#[test]
fn python_dialect_keeps_its_own_rules() -> TestResult {
    let config = tempfile::tempdir()?;
    let dir = project(config.path(), "-proj")?;
    write(
        &dir,
        SESSION,
        &[json!({"type":"user","uuid":"u","entrypoint":"sdk-ts","message":{"content":"sdk"}})],
    )?;
    let files = SessionFiles::with_config_dir(config.path());
    let listed = files.list_sessions(&ListSessionsOptions {
        include_programmatic: false,
        dialect: SummaryDialect::Python,
        ..ListSessionsOptions::default()
    });
    assert_eq!(listed.len(), 1, "Python has no programmatic filter");
    let info = files
        .get_session_info_with(SESSION, None, SummaryDialect::Python)
        .ok_or("missing")?;
    assert_eq!(info.summary, "sdk");
    Ok(())
}

#[test]
fn project_keys_differ_by_dialect_for_non_bmp_paths_over_the_limit() -> TestResult {
    let long = format!("/{}", "\u{1F600}".repeat(120));
    let typescript = project_key_for_directory_in(Some(Path::new(&long)), SummaryDialect::TypeScript)?;
    let python = project_key_for_directory_in(Some(Path::new(&long)), SummaryDialect::Python)?;
    assert_ne!(typescript.as_str(), python.as_str());
    let ascii = "/short/path";
    assert_eq!(
        project_key_for_directory_in(Some(Path::new(ascii)), SummaryDialect::TypeScript)?.as_str(),
        project_key_for_directory_in(Some(Path::new(ascii)), SummaryDialect::Python)?.as_str(),
    );
    Ok(())
}
