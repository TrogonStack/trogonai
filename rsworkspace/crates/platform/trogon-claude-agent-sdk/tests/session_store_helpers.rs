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
    ImportOptions, InMemorySessionStore, MessagePage, SessionEntry, SessionFiles, SessionId, SessionKey,
    SessionListEntry, SessionMutationError, SessionProjectKey, SessionStore, SessionStoreFuture,
    SessionStoreHelperError, SessionStoreOperation, SessionSubpath, SessionSummaryEntry, StoreListOptions,
    delete_session_via_store, fork_session_via_store, get_session_info_from_store, get_session_messages_from_store,
    get_subagent_messages_from_store, import_session_to_store, list_sessions_from_store, list_subagents_from_store,
    project_key_for_directory, rename_session_via_store, tag_session_via_store,
};

type TestResult = Result<(), Box<dyn Error>>;

const SESSION: &str = "123e4567-e89b-42d3-a456-426614174000";
const OTHER: &str = "223e4567-e89b-42d3-a456-426614174000";
const U1: &str = "523e4567-e89b-42d3-a456-426614174001";
const A1: &str = "523e4567-e89b-42d3-a456-426614174002";

fn entry(value: Value) -> Result<SessionEntry, Box<dyn Error>> {
    Ok(SessionEntry::try_from(value)?)
}

fn transcript(session: &str) -> Vec<Value> {
    vec![
        json!({"type":"user","uuid":U1,"parentUuid":null,"sessionId":session,"timestamp":"2024-01-01T00:00:00.000Z",
               "message":{"role":"user","content":"hello world"}}),
        json!({"type":"assistant","uuid":A1,"parentUuid":U1,"sessionId":session,"timestamp":"2024-01-01T00:00:01.000Z",
               "message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"hi"}]}}),
    ]
}

async fn seed(
    store: &impl SessionStore,
    dir: &Path,
    session: &str,
    subpath: Option<&str>,
    lines: &[Value],
) -> Result<SessionKey, Box<dyn Error>> {
    let mut key = SessionKey::new(project_key_for_directory(Some(dir))?, SessionId::from(session))?;
    if let Some(subpath) = subpath {
        key = key.with_subpath(SessionSubpath::new(subpath)?);
    }
    let entries = lines.iter().cloned().map(entry).collect::<Result<Vec<_>, _>>()?;
    store.append(&key, &entries).await?;
    Ok(key)
}

fn dir() -> Result<tempfile::TempDir, Box<dyn Error>> {
    Ok(tempfile::tempdir()?)
}

#[tokio::test]
async fn info_and_messages_read_back_from_the_store() -> TestResult {
    let tmp = dir()?;
    let store = InMemorySessionStore::new();
    seed(&store, tmp.path(), SESSION, None, &transcript(SESSION)).await?;

    let info = get_session_info_from_store(&store, SESSION, Some(tmp.path()))
        .await?
        .ok_or("missing info")?;
    assert_eq!(info.first_prompt.as_deref(), Some("hello world"));
    assert_eq!(info.summary, "hello world");

    let messages = get_session_messages_from_store(&store, SESSION, Some(tmp.path()), MessagePage::default()).await?;
    assert_eq!(messages.len(), 2);

    assert!(
        get_session_info_from_store(&store, "not-a-uuid", Some(tmp.path()))
            .await?
            .is_none()
    );
    assert!(
        get_session_messages_from_store(&store, OTHER, Some(tmp.path()), MessagePage::default())
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn rename_tag_and_delete_go_through_the_store() -> TestResult {
    let tmp = dir()?;
    let store = InMemorySessionStore::new();
    let key = seed(&store, tmp.path(), SESSION, None, &transcript(SESSION)).await?;

    rename_session_via_store(&store, SESSION, "  Named  ", Some(tmp.path())).await?;
    tag_session_via_store(&store, SESSION, Some(" keep "), Some(tmp.path())).await?;
    let info = get_session_info_from_store(&store, SESSION, Some(tmp.path()))
        .await?
        .ok_or("missing")?;
    assert_eq!(info.custom_title.as_deref(), Some("Named"));
    assert_eq!(info.tag.as_deref(), Some("keep"));

    tag_session_via_store(&store, SESSION, None, Some(tmp.path())).await?;
    let info = get_session_info_from_store(&store, SESSION, Some(tmp.path()))
        .await?
        .ok_or("missing")?;
    assert_eq!(info.tag, None);

    assert!(matches!(
        rename_session_via_store(&store, SESSION, " ", Some(tmp.path())).await,
        Err(SessionStoreHelperError::Mutation(SessionMutationError::EmptyTitle))
    ));
    assert!(matches!(
        tag_session_via_store(&store, SESSION, Some(" "), Some(tmp.path())).await,
        Err(SessionStoreHelperError::Mutation(SessionMutationError::EmptyTag))
    ));
    assert!(matches!(
        delete_session_via_store(&store, "bad", Some(tmp.path())).await,
        Err(SessionStoreHelperError::Mutation(
            SessionMutationError::InvalidSessionId(_)
        ))
    ));

    delete_session_via_store(&store, SESSION, Some(tmp.path())).await?;
    assert!(store.load(&key).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn fork_copies_the_conversation_under_a_new_id() -> TestResult {
    let tmp = dir()?;
    let store = InMemorySessionStore::new();
    seed(&store, tmp.path(), SESSION, None, &transcript(SESSION)).await?;

    let forked = fork_session_via_store(&store, SESSION, Some(tmp.path()), Some(U1), Some("Branch")).await?;
    assert_ne!(forked.session_id.as_str(), SESSION);
    let messages = get_session_messages_from_store(
        &store,
        forked.session_id.as_str(),
        Some(tmp.path()),
        MessagePage::default(),
    )
    .await?;
    assert_eq!(messages.len(), 1);
    let info = get_session_info_from_store(&store, forked.session_id.as_str(), Some(tmp.path()))
        .await?
        .ok_or("missing")?;
    assert_eq!(info.custom_title.as_deref(), Some("Branch"));

    assert!(matches!(
        fork_session_via_store(&store, OTHER, Some(tmp.path()), None, None).await,
        Err(SessionStoreHelperError::Mutation(SessionMutationError::NotFound { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn list_uses_summaries_with_pagination() -> TestResult {
    let tmp = dir()?;
    let store = InMemorySessionStore::new();
    seed(&store, tmp.path(), SESSION, None, &transcript(SESSION)).await?;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    seed(&store, tmp.path(), OTHER, None, &transcript(OTHER)).await?;

    let options = StoreListOptions {
        directory: Some(tmp.path().to_path_buf()),
        ..StoreListOptions::default()
    };
    let all = list_sessions_from_store(&store, &options).await?;
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].session_id.as_str(), OTHER);

    let page = list_sessions_from_store(
        &store,
        &StoreListOptions {
            limit: Some(1),
            offset: 1,
            ..options
        },
    )
    .await?;
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].session_id.as_str(), SESSION);
    Ok(())
}

/// Delegates to an in-memory store but advertises only the listed optional operations.
struct Limited {
    inner: InMemorySessionStore,
    list: bool,
    summaries: bool,
    stale_summaries: bool,
}

impl SessionStore for Limited {
    fn supports(&self, operation: SessionStoreOperation) -> bool {
        match operation {
            SessionStoreOperation::List => self.list,
            SessionStoreOperation::ListSummaries => self.summaries,
            _ => true,
        }
    }
    fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        self.inner.append(key, entries)
    }
    fn load<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        self.inner.load(key)
    }
    fn list_sessions<'a>(&'a self, project: &'a SessionProjectKey) -> SessionStoreFuture<'a, Vec<SessionListEntry>> {
        self.inner.list_sessions(project)
    }
    fn list_session_summaries<'a>(
        &'a self,
        project: &'a SessionProjectKey,
    ) -> SessionStoreFuture<'a, Vec<SessionSummaryEntry>> {
        if self.stale_summaries {
            // Summaries that know nothing: every session must be gap-filled from `load`.
            return Box::pin(async { Ok(Vec::new()) });
        }
        self.inner.list_session_summaries(project)
    }
    fn list_subkeys<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Vec<String>> {
        self.inner.list_subkeys(key)
    }
}

#[tokio::test]
async fn list_gap_fills_sessions_missing_from_summaries() -> TestResult {
    let tmp = dir()?;
    let store = Limited {
        inner: InMemorySessionStore::new(),
        list: true,
        summaries: true,
        stale_summaries: true,
    };
    seed(&store, tmp.path(), SESSION, None, &transcript(SESSION)).await?;
    let options = StoreListOptions {
        directory: Some(tmp.path().to_path_buf()),
        ..StoreListOptions::default()
    };
    let listed = list_sessions_from_store(&store, &options).await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].first_prompt.as_deref(), Some("hello world"));
    Ok(())
}

#[tokio::test]
async fn list_falls_back_to_loading_each_session_without_summaries() -> TestResult {
    let tmp = dir()?;
    let store = Limited {
        inner: InMemorySessionStore::new(),
        list: true,
        summaries: false,
        stale_summaries: false,
    };
    seed(&store, tmp.path(), SESSION, None, &transcript(SESSION)).await?;
    let options = StoreListOptions {
        directory: Some(tmp.path().to_path_buf()),
        ..StoreListOptions::default()
    };
    assert_eq!(list_sessions_from_store(&store, &options).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn list_requires_a_listing_capability() -> TestResult {
    let tmp = dir()?;
    let store = Limited {
        inner: InMemorySessionStore::new(),
        list: false,
        summaries: false,
        stale_summaries: false,
    };
    let options = StoreListOptions {
        directory: Some(tmp.path().to_path_buf()),
        ..StoreListOptions::default()
    };
    assert!(matches!(
        list_sessions_from_store(&store, &options).await,
        Err(SessionStoreHelperError::UnsupportedStore(_))
    ));
    Ok(())
}

#[tokio::test]
async fn subagents_list_and_read_with_parent_metadata() -> TestResult {
    let tmp = dir()?;
    let store = InMemorySessionStore::new();
    seed(&store, tmp.path(), SESSION, None, &transcript(SESSION)).await?;
    let mut sub = transcript(SESSION);
    sub.push(json!({"type":"agent_metadata","agentType":"general"}));
    seed(&store, tmp.path(), SESSION, Some("subagents/agent-abc"), &sub).await?;

    assert_eq!(
        list_subagents_from_store(&store, SESSION, Some(tmp.path())).await?,
        vec!["abc".to_owned()]
    );
    let messages =
        get_subagent_messages_from_store(&store, SESSION, "abc", Some(tmp.path()), MessagePage::default()).await?;
    assert_eq!(messages.len(), 2);
    assert!(
        get_subagent_messages_from_store(&store, SESSION, "missing", Some(tmp.path()), MessagePage::default())
            .await?
            .is_empty()
    );
    assert!(
        get_subagent_messages_from_store(&store, SESSION, "", Some(tmp.path()), MessagePage::default())
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn import_replays_the_transcript_subagents_and_sidecars() -> TestResult {
    let config = tempfile::tempdir()?;
    let project = config.path().join("projects").join("-proj");
    fs::create_dir_all(project.join(SESSION).join("subagents"))?;
    let body: String = transcript(SESSION).iter().map(|line| format!("{line}\n\n")).collect();
    fs::write(project.join(format!("{SESSION}.jsonl")), body)?;
    let sub: String = transcript(SESSION).iter().map(|line| format!("{line}\n")).collect();
    fs::write(project.join(SESSION).join("subagents").join("agent-abc.jsonl"), sub)?;
    fs::write(
        project.join(SESSION).join("subagents").join("agent-abc.meta.json"),
        r#"{"agentType":"general","type":"stray"}"#,
    )?;
    let files = SessionFiles::with_config_dir(config.path());
    let store = InMemorySessionStore::new();

    import_session_to_store(
        &files,
        SESSION,
        &store,
        &ImportOptions {
            batch_size: 1,
            ..ImportOptions::default()
        },
    )
    .await?;

    let main = SessionKey::new(SessionProjectKey::new("-proj")?, SessionId::from(SESSION))?;
    assert_eq!(store.load(&main).await?.map_or(0, |entries| entries.len()), 2);
    let sub_key = main.with_subpath(SessionSubpath::new("subagents/agent-abc")?);
    let loaded = store.load(&sub_key).await?.ok_or("no subagent")?;
    assert_eq!(loaded.len(), 3);
    let last = loaded.last().ok_or("empty")?;
    assert_eq!(last.as_object().get("type"), Some(&json!("agent_metadata")));
    assert_eq!(last.as_object().get("agentType"), Some(&json!("general")));

    let skipped = InMemorySessionStore::new();
    import_session_to_store(
        &files,
        SESSION,
        &skipped,
        &ImportOptions {
            include_subagents: false,
            ..ImportOptions::default()
        },
    )
    .await?;
    assert_eq!(skipped.size()?, 1);

    assert!(matches!(
        import_session_to_store(&files, OTHER, &skipped, &ImportOptions::default()).await,
        Err(SessionStoreHelperError::Mutation(SessionMutationError::NotFound { .. }))
    ));
    assert!(matches!(
        import_session_to_store(&files, "bad", &skipped, &ImportOptions::default()).await,
        Err(SessionStoreHelperError::Mutation(
            SessionMutationError::InvalidSessionId(_)
        ))
    ));
    Ok(())
}
