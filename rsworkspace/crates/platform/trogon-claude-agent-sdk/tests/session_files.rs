#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Fixture transcripts are exact upstream-shaped JSONL lines."
    )
)]

use std::{error::Error, fs, path::Path};

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    ListSessionsOptions, MessagePage, SessionFiles, SessionMessageKind, project_key_for_directory,
};

type TestResult = Result<(), Box<dyn Error>>;

const SESSION: &str = "123e4567-e89b-42d3-a456-426614174000";
const OTHER: &str = "223e4567-e89b-42d3-a456-426614174001";

fn jsonl(lines: &[Value]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

fn project(config: &Path, name: &str) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let dir = config.join("projects").join(name);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn user(uuid: &str, parent: Option<&str>, text: &str) -> Value {
    json!({"type":"user","uuid":uuid,"parentUuid":parent,"sessionId":SESSION,
           "timestamp":"2024-01-01T00:00:00.000Z","message":{"role":"user","content":text}})
}

fn assistant(uuid: &str, parent: Option<&str>, message_id: &str, content: Value) -> Value {
    json!({"type":"assistant","uuid":uuid,"parentUuid":parent,"sessionId":SESSION,
           "timestamp":"2024-01-01T00:00:01.000Z",
           "message":{"id":message_id,"role":"assistant","content":content}})
}

#[test]
fn listing_reads_metadata_from_head_and_tail_and_orders_newest_first() -> TestResult {
    let config = tempfile::tempdir()?;
    let dir = project(config.path(), "-proj")?;
    fs::write(
        dir.join(format!("{SESSION}.jsonl")),
        jsonl(&[
            json!({"type":"user","uuid":"a","cwd":"/work","gitBranch":"main","timestamp":"2024-01-01T00:00:00.000Z",
                   "message":{"content":"first question"}}),
            json!({"type":"custom-title","customTitle":"Named"}),
        ]) + "{\"type\":\"tag\",\"tag\":\"keep\",\"sessionId\":\"x\"}\n",
    )?;
    fs::write(
        dir.join(format!("{OTHER}.jsonl")),
        jsonl(&[json!({"type":"user","uuid":"b","message":{"content":"other"}})]),
    )?;
    fs::write(dir.join("not-a-uuid.jsonl"), "{}\n")?;
    fs::write(dir.join(format!("{SESSION}.txt")), "x")?;
    let sidechain = "323e4567-e89b-42d3-a456-426614174002";
    fs::write(
        dir.join(format!("{sidechain}.jsonl")),
        jsonl(&[json!({"type":"user","isSidechain":true,"message":{"content":"hidden"}})]),
    )?;
    let empty = "423e4567-e89b-42d3-a456-426614174003";
    fs::write(dir.join(format!("{empty}.jsonl")), "")?;

    let files = SessionFiles::with_config_dir(config.path());
    let sessions = files.list_sessions(&ListSessionsOptions::default());
    assert_eq!(sessions.len(), 2, "sidechain, empty, and invalid names are skipped");
    let named = sessions
        .iter()
        .find(|info| info.session_id.as_str() == SESSION)
        .ok_or("missing session")?;
    assert_eq!(named.summary, "Named");
    assert_eq!(named.custom_title.as_deref(), Some("Named"));
    assert_eq!(named.first_prompt.as_deref(), Some("first question"));
    assert_eq!(named.git_branch.as_deref(), Some("main"));
    assert_eq!(named.cwd.as_deref(), Some("/work"));
    assert_eq!(named.tag.as_deref(), Some("keep"));
    assert_eq!(named.created_at, Some(1_704_067_200_000));
    assert!(named.file_size.is_some_and(|size| size > 0));
    assert!(
        sessions
            .windows(2)
            .all(|pair| pair[0].last_modified >= pair[1].last_modified)
    );

    let page = files.list_sessions(&ListSessionsOptions {
        limit: Some(1),
        offset: 1,
        ..ListSessionsOptions::default()
    });
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].session_id, sessions[1].session_id);
    Ok(())
}

#[test]
fn listing_by_directory_uses_the_sanitized_project_directory_and_its_path_as_cwd() -> TestResult {
    let config = tempfile::tempdir()?;
    let work = tempfile::tempdir()?;
    let key = project_key_for_directory(Some(work.path()))?;
    let dir = project(config.path(), key.as_str())?;
    fs::write(
        dir.join(format!("{SESSION}.jsonl")),
        jsonl(&[json!({"type":"user","uuid":"a","message":{"content":"hello"}})]),
    )?;
    let files = SessionFiles::with_config_dir(config.path());
    let sessions = files.list_sessions(&ListSessionsOptions {
        directory: Some(work.path().to_path_buf()),
        include_worktrees: false,
        ..ListSessionsOptions::default()
    });
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].cwd.is_some(), "the project path is the cwd fallback");
    let info = files
        .get_session_info(SESSION, Some(work.path()))
        .ok_or("missing info")?;
    assert_eq!(info.summary, "hello");
    assert!(files.get_session_info(SESSION, None).is_some());
    assert!(files.get_session_info("not-a-uuid", None).is_none());
    assert!(files.get_session_info(OTHER, None).is_none());
    Ok(())
}

#[test]
fn long_project_paths_fall_back_to_the_shared_prefix_when_hashes_differ() -> TestResult {
    let config = tempfile::tempdir()?;
    let long_path = format!("/{}", "segment/".repeat(40));
    let prefix: String = long_path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(200)
        .collect();
    let dir = project(config.path(), &format!("{prefix}-someotherhash"))?;
    let files = SessionFiles::with_config_dir(config.path());
    assert_eq!(files.find_project_dir(&long_path).as_deref(), Some(dir.as_path()));
    assert_eq!(files.find_project_dir("/short/path"), None);
    Ok(())
}

#[test]
fn messages_follow_the_parent_chain_and_skip_meta_and_sidechain_entries() -> TestResult {
    let config = tempfile::tempdir()?;
    let dir = project(config.path(), "-proj")?;
    let mut meta = user("m", Some("u1"), "meta");
    meta["isMeta"] = json!(true);
    fs::write(
        dir.join(format!("{SESSION}.jsonl")),
        jsonl(&[
            user("u1", None, "hello"),
            json!({"type":"progress","uuid":"p1","parentUuid":"u1"}),
            assistant("a1", Some("p1"), "msg_1", json!([{"type":"text","text":"hi"}])),
            meta,
            user("dead-branch", Some("u1"), "abandoned"),
            user("u2", Some("a1"), "again"),
        ]),
    )?;
    fs::write(dir.join("corrupt.jsonl"), "garbage")?;
    let files = SessionFiles::with_config_dir(config.path());
    let messages = files.get_session_messages(SESSION, None, MessagePage::default())?;
    let uuids: Vec<&str> = messages.iter().map(|message| message.uuid.as_str()).collect();
    assert_eq!(uuids, ["u1", "a1", "u2"]);
    assert_eq!(messages[0].kind, SessionMessageKind::User);
    assert_eq!(messages[1].kind, SessionMessageKind::Assistant);
    assert_eq!(messages[0].session_id, SESSION);
    assert_eq!(messages[0].message["content"], "hello");
    assert!(messages.iter().all(|message| message.parent_tool_use_id.is_none()));

    let paged = files.get_session_messages(
        SESSION,
        None,
        MessagePage {
            limit: Some(1),
            offset: 1,
        },
    )?;
    assert_eq!(paged.len(), 1);
    assert_eq!(paged[0].uuid, "a1");
    let tail = files.get_session_messages(SESSION, None, MessagePage { limit: None, offset: 2 })?;
    assert_eq!(tail.len(), 1);
    let zero_limit = files.get_session_messages(
        SESSION,
        None,
        MessagePage {
            limit: Some(0),
            offset: 0,
        },
    )?;
    assert_eq!(zero_limit.len(), 3, "a zero limit means no limit");
    assert!(
        files
            .get_session_messages(OTHER, None, MessagePage::default())?
            .is_empty()
    );
    Ok(())
}

#[test]
fn parallel_tool_calls_and_results_are_recovered_around_the_walked_branch() -> TestResult {
    let config = tempfile::tempdir()?;
    let dir = project(config.path(), "-proj")?;
    let tool_result = |uuid: &str, parent: &str, call: &str, stamp: &str| {
        json!({"type":"user","uuid":uuid,"parentUuid":parent,"sessionId":SESSION,"timestamp":stamp,
               "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":call,"content":"ok"}]}})
    };
    fs::write(
        dir.join(format!("{SESSION}.jsonl")),
        jsonl(&[
            user("u1", None, "run both"),
            assistant(
                "a1",
                Some("u1"),
                "msg_1",
                json!([{"type":"tool_use","id":"call1","name":"X","input":{}}]),
            ),
            assistant(
                "a2",
                Some("a1"),
                "msg_1",
                json!([{"type":"tool_use","id":"call2","name":"Y","input":{}}]),
            ),
            tool_result("r1", "a1", "call1", "2024-01-01T00:00:02.000Z"),
            tool_result("r2", "a2", "call2", "2024-01-01T00:00:03.000Z"),
            assistant("a3", Some("r2"), "msg_2", json!([{"type":"text","text":"done"}])),
        ]),
    )?;
    let files = SessionFiles::with_config_dir(config.path());
    let uuids: Vec<String> = files
        .get_session_messages(SESSION, None, MessagePage::default())?
        .into_iter()
        .map(|message| message.uuid)
        .collect();
    assert_eq!(uuids, ["u1", "a1", "a2", "r1", "r2", "a3"]);
    Ok(())
}

#[test]
fn subagents_are_listed_recursively_and_carry_their_sidecar_parent_ids() -> TestResult {
    let config = tempfile::tempdir()?;
    let dir = project(config.path(), "-proj")?;
    fs::write(dir.join(format!("{SESSION}.jsonl")), jsonl(&[user("u1", None, "hi")]))?;
    let subagents = dir.join(SESSION).join("subagents");
    fs::create_dir_all(subagents.join("workflows").join("run1"))?;
    fs::write(
        subagents.join("agent-beta.jsonl"),
        jsonl(&[
            user("s1", None, "task"),
            assistant("s2", Some("s1"), "msg_s", json!([{"type":"text","text":"result"}])),
        ]),
    )?;
    fs::write(
        subagents.join("agent-beta.meta.json"),
        json!({"toolUseId":"toolu_1","parentAgentId":"alpha"}).to_string(),
    )?;
    fs::write(subagents.join("agent-alpha.jsonl"), jsonl(&[user("t1", None, "x")]))?;
    fs::write(
        subagents.join("workflows").join("run1").join("agent-gamma.jsonl"),
        jsonl(&[user("g1", None, "y")]),
    )?;
    fs::write(subagents.join("ignore.jsonl"), "{}")?;

    let files = SessionFiles::with_config_dir(config.path());
    assert_eq!(files.list_subagents(SESSION, None), ["alpha", "beta", "gamma"]);
    assert!(files.list_subagents(OTHER, None).is_empty());
    let messages = files.get_subagent_messages(SESSION, "beta", None, MessagePage::default());
    assert_eq!(messages.len(), 2);
    assert!(
        messages
            .iter()
            .all(|message| message.parent_tool_use_id.as_deref() == Some("toolu_1"))
    );
    assert!(
        messages
            .iter()
            .all(|message| message.parent_agent_id.as_deref() == Some("alpha"))
    );
    let no_meta = files.get_subagent_messages(SESSION, "gamma", None, MessagePage::default());
    assert_eq!(no_meta.len(), 1);
    assert_eq!(no_meta[0].parent_tool_use_id, None);
    assert!(
        files
            .get_subagent_messages(SESSION, "missing", None, MessagePage::default())
            .is_empty()
    );
    assert!(
        files
            .get_subagent_messages(SESSION, "", None, MessagePage::default())
            .is_empty()
    );
    Ok(())
}

#[test]
fn the_project_key_hashes_paths_longer_than_two_hundred_characters() -> TestResult {
    let short = project_key_for_directory(Some(Path::new("/definitely/not/here")))?;
    assert_eq!(short.as_str(), "-definitely-not-here");
    let long = format!("/{}", "ab/".repeat(100));
    let key = project_key_for_directory(Some(Path::new(&long)))?;
    assert!(key.as_str().len() > 200);
    assert!(key.as_str().starts_with(&"-ab".repeat(66)));
    Ok(())
}
