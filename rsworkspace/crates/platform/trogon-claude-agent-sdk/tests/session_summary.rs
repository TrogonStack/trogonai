#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Summary contracts use exact transcript entries and expected sidecar data."
    )
)]

use std::{error::Error, num::NonZeroUsize, path::PathBuf, sync::Arc, time::Duration};

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    InMemorySessionStore, MirrorSizeEstimate, SessionEntry, SessionId, SessionKey, SessionProjectKey, SessionStore,
    SessionStoreFlush, SessionStoreOptions, SummaryDialect, TranscriptMirror, fold_session_summary,
    summary_entry_to_sdk_info,
};

type TestResult = Result<(), Box<dyn Error>>;

const SESSION: &str = "123e4567-e89b-42d3-a456-426614174000";

fn key() -> Result<SessionKey, Box<dyn Error>> {
    Ok(SessionKey::new(
        SessionProjectKey::new("project")?,
        SessionId::from(SESSION),
    )?)
}

fn entries(values: Vec<Value>) -> Result<Vec<SessionEntry>, Box<dyn Error>> {
    values
        .into_iter()
        .map(|value| SessionEntry::try_from(value).map_err(Into::into))
        .collect()
}

#[test]
fn folding_latches_set_once_fields_and_overwrites_last_wins_fields() -> TestResult {
    let key = key()?;
    let first = fold_session_summary(
        SummaryDialect::Python,
        None,
        &key,
        &entries(vec![
            json!({"type":"user","timestamp":"2024-01-01T00:00:00.000Z","cwd":"/a","gitBranch":"one",
                   "message":{"content":"first prompt"}}),
            json!({"type":"user","timestamp":"2024-02-01T00:00:00.000Z","cwd":"/b","gitBranch":"two",
                   "message":{"content":"second prompt"},"customTitle":"title"}),
        ])?,
        Some(42),
    );
    assert_eq!(first.modified_at_ms, 42);
    assert_eq!(first.data["created_at"], 1_704_067_200_000_i64);
    assert_eq!(first.data["cwd"], "/a");
    assert_eq!(first.data["git_branch"], "two");
    assert_eq!(first.data["first_prompt"], "first prompt");
    assert_eq!(first.data["first_prompt_locked"], true);
    assert_eq!(first.data["custom_title"], "title");
    assert_eq!(first.data["is_sidechain"], false);

    let next = fold_session_summary(
        SummaryDialect::Python,
        Some(&first),
        &key,
        &entries(vec![json!({"type":"tag","tag":"keep"}), json!({"type":"tag","tag":""})])?,
        None,
    );
    assert_eq!(next.modified_at_ms, 42, "the fold keeps the previous write time");
    assert!(next.data.get("tag").is_none(), "an empty tag clears it");
    Ok(())
}

#[test]
fn the_typescript_layout_uses_camel_case_keys_and_honors_relocated_entries() -> TestResult {
    let key = key()?;
    let summary = fold_session_summary(
        SummaryDialect::TypeScript,
        None,
        &key,
        &entries(vec![
            json!({"type":"user","cwd":"/a","isSidechain":true,"message":{"content":"<bash-input>ls -la</bash-input>"}}),
            json!({"type":"relocated","relocatedCwd":"/moved"}),
            json!({"summary":"hint"}),
        ])?,
        Some(1),
    );
    assert_eq!(summary.data["isSidechain"], true);
    assert_eq!(summary.data["firstPrompt"], "! ls -la");
    assert_eq!(summary.data["firstPromptLocked"], true);
    assert_eq!(summary.data["cwd"], "/moved");
    assert_eq!(summary.data["summaryHint"], "hint");
    assert!(summary_entry_to_sdk_info(SummaryDialect::TypeScript, &summary, None).is_none());

    let python = fold_session_summary(
        SummaryDialect::Python,
        None,
        &key,
        &entries(vec![
            json!({"type":"user","message":{"content":"<bash-input>ls</bash-input>"}}),
            json!({"type":"relocated","relocatedCwd":"/moved"}),
        ])?,
        Some(1),
    );
    assert_eq!(python.data["first_prompt"], "<bash-input>ls</bash-input>");
    assert!(python.data.get("cwd").is_none());
    Ok(())
}

#[test]
fn slash_commands_only_fall_back_and_noise_prompts_are_skipped() -> TestResult {
    let key = key()?;
    for dialect in [SummaryDialect::TypeScript, SummaryDialect::Python] {
        let summary = fold_session_summary(
            dialect,
            None,
            &key,
            &entries(vec![
                json!({"type":"user","isMeta":true,"message":{"content":"meta"}}),
                json!({"type":"user","message":{"content":[{"type":"tool_result","content":"x"}]}}),
                json!({"type":"user","message":{"content":"<command-name>/clear</command-name>"}}),
                json!({"type":"user","message":{"content":"[Request interrupted by user]"}}),
            ])?,
            Some(1),
        );
        let info = summary_entry_to_sdk_info(dialect, &summary, Some("/project"))
            .ok_or("a command-only session still has a summary")?;
        assert_eq!(info.summary, "/clear");
        assert_eq!(info.first_prompt.as_deref(), Some("/clear"));
        assert_eq!(info.cwd.as_deref(), Some("/project"));
    }
    Ok(())
}

#[test]
fn long_prompts_truncate_with_an_ellipsis_per_dialect() -> TestResult {
    let key = key()?;
    let long = format!("{} tail", "a".repeat(199));
    let python = fold_session_summary(
        SummaryDialect::Python,
        None,
        &key,
        &entries(vec![json!({"type":"user","message":{"content":long}})])?,
        Some(1),
    );
    assert_eq!(python.data["first_prompt"], format!("{}\u{2026}", "a".repeat(199)));
    let emoji = "\u{1F600}".repeat(150);
    let typescript = fold_session_summary(
        SummaryDialect::TypeScript,
        None,
        &key,
        &entries(vec![json!({"type":"user","message":{"content":emoji}})])?,
        Some(1),
    );
    // 200 UTF-16 units hold 100 emoji.
    assert_eq!(
        typescript.data["firstPrompt"],
        format!("{}\u{2026}", "\u{1F600}".repeat(100))
    );
    Ok(())
}

#[tokio::test]
async fn the_in_memory_store_keeps_summaries_in_step_with_listings() -> TestResult {
    let store = InMemorySessionStore::new();
    let key = key()?;
    store
        .append(
            &key,
            &entries(vec![
                json!({"type":"user","message":{"content":"hello"},"customTitle":"named"}),
            ])?,
        )
        .await?;
    let summaries = store.list_session_summaries(key.project_key()).await?;
    assert_eq!(summaries.len(), 1);
    let listed = store.list_sessions(key.project_key()).await?;
    assert_eq!(summaries[0].modified_at_ms, listed[0].modified_at_ms);
    let info = summary_entry_to_sdk_info(SummaryDialect::TypeScript, &summaries[0], None).ok_or("missing info")?;
    assert_eq!(info.summary, "named");
    assert_eq!(info.custom_title.as_deref(), Some("named"));
    assert_eq!(info.first_prompt.as_deref(), Some("hello"));
    store.delete(&key).await?;
    assert!(store.list_session_summaries(key.project_key()).await?.is_empty());
    Ok(())
}

fn mirror_path() -> PathBuf {
    PathBuf::from(format!("/memory-config/projects/project/{SESSION}.jsonl"))
}

#[tokio::test(start_paused = true)]
async fn eager_flush_appends_every_frame_without_waiting_for_a_flush() -> TestResult {
    let store = Arc::new(InMemorySessionStore::new());
    let mut options = SessionStoreOptions::from(store.clone());
    options.flush = SessionStoreFlush::Eager;
    let mirror = TranscriptMirror::new(&options, PathBuf::from("/memory-config/projects"));
    mirror.enqueue(&mirror_path(), entries(vec![json!({"type":"user"})])?)?;
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert_eq!(store.load(&key()?).await?.map(|loaded| loaded.len()), Some(1));

    let batched = Arc::new(InMemorySessionStore::new());
    let mirror = TranscriptMirror::new(
        &SessionStoreOptions::from(batched.clone()),
        PathBuf::from("/memory-config/projects"),
    );
    mirror.enqueue(&mirror_path(), entries(vec![json!({"type":"user"})])?)?;
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert_eq!(batched.load(&key()?).await?, None);
    mirror.flush().await?;
    assert_eq!(batched.load(&key()?).await?.map(|loaded| loaded.len()), Some(1));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_python_size_estimate_counts_ascii_escapes_where_typescript_counts_code_units() -> TestResult {
    for (estimate, drains_eagerly) in [
        (MirrorSizeEstimate::TypeScript, false),
        (MirrorSizeEstimate::Python, true),
    ] {
        let store = Arc::new(InMemorySessionStore::new());
        let mut options = SessionStoreOptions::from(store.clone());
        options.size_estimate = estimate;
        options.limits.max_pending_bytes = NonZeroUsize::new(300).ok_or("zero limit")?;
        let mirror = TranscriptMirror::new(&options, PathBuf::from("/memory-config/projects"));
        mirror.enqueue(
            &mirror_path(),
            entries(vec![json!({"type":"user","text":"\u{e9}".repeat(100)})])?,
        )?;
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(store.load(&key()?).await?.is_some(), drains_eagerly, "{estimate:?}");
    }
    Ok(())
}
