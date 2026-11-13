#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "The conformance checks append literal transcript entries shaped like upstream fixtures."
    )
)]

use std::{future::Future, sync::Arc};

use serde_json::{Value, json};

use crate::{
    SessionEntry, SessionId, SessionKey, SessionProjectKey, SessionStore, SessionStoreOperation, SessionSubpath,
    SummaryDialect, fold_session_summary,
};

/// A violated [`SessionStore`] contract, naming the numbered upstream check that failed.
#[derive(Debug, thiserror::Error)]
#[error("session store conformance check {check} failed: {detail}")]
pub struct ConformanceError {
    pub check: u8,
    pub detail: String,
}

/// Which optional operations to skip and which summary layout the store folds.
///
/// Equivalent of Python's `skip_optional`; a store that does not report an optional operation through
/// [`SessionStore::supports`] is skipped as well.
#[derive(Clone, Debug, Default)]
pub struct ConformanceOptions {
    pub skip_optional: Vec<SessionStoreOperation>,
    pub dialect: SummaryDialect,
}

type Check = Result<(), ConformanceError>;

fn fail(check: u8, detail: impl Into<String>) -> ConformanceError {
    ConformanceError {
        check,
        detail: detail.into(),
    }
}

fn expect(check: u8, condition: bool, detail: &str) -> Check {
    if condition { Ok(()) } else { Err(fail(check, detail)) }
}

fn store_error(check: u8) -> impl Fn(crate::SessionStoreError) -> ConformanceError {
    move |error| {
        let mut detail = error.to_string();
        let mut source = std::error::Error::source(&error);
        while let Some(next) = source {
            detail.push_str(": ");
            detail.push_str(&next.to_string());
            source = next.source();
        }
        fail(check, detail)
    }
}

fn key(project: &str, session: &str) -> Result<SessionKey, ConformanceError> {
    SessionKey::new(
        SessionProjectKey::new(project).map_err(|error| fail(0, error.to_string()))?,
        SessionId::from(session),
    )
    .map_err(|error| fail(0, error.to_string()))
}

fn sub(base: &SessionKey, subpath: &str) -> Result<SessionKey, ConformanceError> {
    Ok(base
        .clone()
        .with_subpath(SessionSubpath::new(subpath).map_err(|error| fail(0, error.to_string()))?))
}

fn project(name: &str) -> Result<SessionProjectKey, ConformanceError> {
    SessionProjectKey::new(name).map_err(|error| fail(0, error.to_string()))
}

/// Builds an entry like upstream's `_e`: `type` is required and otherwise irrelevant to the contracts.
fn entry(fields: Value) -> Result<SessionEntry, ConformanceError> {
    let mut object = serde_json::Map::new();
    object.insert("type".into(), "x".into());
    if let Value::Object(fields) = fields {
        object.extend(fields);
    }
    Ok(SessionEntry::new(object))
}

fn entries(values: Vec<Value>) -> Result<Vec<SessionEntry>, ConformanceError> {
    values.into_iter().map(entry).collect()
}

/// Asserts the 14 behavioral contracts every [`SessionStore`] must satisfy (upstream
/// `run_session_store_conformance`). `make_store` is called once per contract so each starts from an empty store.
pub async fn run_session_store_conformance<F, Fut>(make_store: F, options: ConformanceOptions) -> Check
where
    F: Fn() -> Fut,
    Fut: Future<Output = Arc<dyn SessionStore>>,
{
    let probe = make_store().await;
    let enabled = |operation| !options.skip_optional.contains(&operation) && probe.supports(operation);
    let has_list_sessions = enabled(SessionStoreOperation::List);
    let has_summaries = enabled(SessionStoreOperation::ListSummaries);
    let has_delete = enabled(SessionStoreOperation::Delete);
    let has_subkeys = enabled(SessionStoreOperation::ListSubkeys);
    let main = key("proj", "sess")?;

    // 1. append then load returns the same entries in the same order
    let store = make_store().await;
    let first = entries(vec![json!({"uuid": "b", "n": 1}), json!({"uuid": "a", "n": 2})])?;
    store.append(&main, &first).await.map_err(store_error(1))?;
    expect(
        1,
        store.load(&main).await.map_err(store_error(1))? == Some(first),
        "load did not return the appended entries in order",
    )?;

    // 2. load of an unknown key returns None
    let store = make_store().await;
    expect(
        2,
        store
            .load(&key("proj", "nope")?)
            .await
            .map_err(store_error(2))?
            .is_none(),
        "unknown session returned entries",
    )?;
    store
        .append(&main, &entries(vec![json!({"uuid": "x", "n": 1})])?)
        .await
        .map_err(store_error(2))?;
    expect(
        2,
        store
            .load(&sub(&main, "nope")?)
            .await
            .map_err(store_error(2))?
            .is_none(),
        "unknown subpath returned entries",
    )?;

    // 3. multiple appends preserve call order
    let store = make_store().await;
    let batches = [
        entries(vec![json!({"uuid": "z", "n": 1})])?,
        entries(vec![json!({"uuid": "a", "n": 2}), json!({"uuid": "m", "n": 3})])?,
        entries(vec![json!({"uuid": "b", "n": 4})])?,
    ];
    for batch in &batches {
        store.append(&main, batch).await.map_err(store_error(3))?;
    }
    expect(
        3,
        store.load(&main).await.map_err(store_error(3))? == Some(batches.concat()),
        "append calls were reordered",
    )?;

    // 4. append of an empty batch is a no-op
    let store = make_store().await;
    let only = entries(vec![json!({"uuid": "a", "n": 1})])?;
    store.append(&main, &only).await.map_err(store_error(4))?;
    store.append(&main, &[]).await.map_err(store_error(4))?;
    expect(
        4,
        store.load(&main).await.map_err(store_error(4))? == Some(only),
        "empty append changed the transcript",
    )?;

    // 5. subpath keys are stored independently of the main transcript
    let store = make_store().await;
    let child = sub(&main, "subagents/agent-1")?;
    let main_entries = entries(vec![json!({"uuid": "m", "n": 1})])?;
    let child_entries = entries(vec![json!({"uuid": "s", "n": 1})])?;
    store.append(&main, &main_entries).await.map_err(store_error(5))?;
    store.append(&child, &child_entries).await.map_err(store_error(5))?;
    expect(
        5,
        store.load(&main).await.map_err(store_error(5))? == Some(main_entries)
            && store.load(&child).await.map_err(store_error(5))? == Some(child_entries),
        "subpath and main transcripts were mixed",
    )?;

    // 6. project key isolation
    let store = make_store().await;
    let (project_a, project_b) = (key("A", "s1")?, key("B", "s1")?);
    let (from_a, from_b) = (
        entries(vec![json!({"from": "A"})])?,
        entries(vec![json!({"from": "B"})])?,
    );
    store.append(&project_a, &from_a).await.map_err(store_error(6))?;
    store.append(&project_b, &from_b).await.map_err(store_error(6))?;
    expect(
        6,
        store.load(&project_a).await.map_err(store_error(6))? == Some(from_a)
            && store.load(&project_b).await.map_err(store_error(6))? == Some(from_b),
        "projects shared a transcript",
    )?;
    if has_list_sessions {
        for name in ["A", "B"] {
            expect(
                6,
                store
                    .list_sessions(&project(name)?)
                    .await
                    .map_err(store_error(6))?
                    .len()
                    == 1,
                "project listing leaked another project",
            )?;
        }
    }

    if has_list_sessions {
        // 7. list_sessions returns the session ids of the project
        let store = make_store().await;
        for (project_name, session) in [("proj", "a"), ("proj", "b"), ("other", "c")] {
            store
                .append(&key(project_name, session)?, &entries(vec![json!({"n": 1})])?)
                .await
                .map_err(store_error(7))?;
        }
        let sessions = store.list_sessions(&project("proj")?).await.map_err(store_error(7))?;
        let mut ids: Vec<_> = sessions.iter().map(|session| session.session_id.as_str()).collect();
        ids.sort_unstable();
        expect(7, ids == ["a", "b"], "list_sessions returned the wrong sessions")?;
        expect(
            7,
            sessions
                .iter()
                .all(|session| session.modified_at_ms > 1_000_000_000_000),
            "modified time is not epoch milliseconds",
        )?;
        expect(
            7,
            store
                .list_sessions(&project("never-appended-project")?)
                .await
                .map_err(store_error(7))?
                .is_empty(),
            "unknown project listed sessions",
        )?;

        // 8. list_sessions excludes subagent subpaths
        let store = make_store().await;
        let session = key("proj", "main")?;
        store
            .append(&session, &entries(vec![json!({"n": 1})])?)
            .await
            .map_err(store_error(8))?;
        store
            .append(&sub(&session, "subagents/agent-1")?, &entries(vec![json!({"n": 1})])?)
            .await
            .map_err(store_error(8))?;
        let sessions = store.list_sessions(&project("proj")?).await.map_err(store_error(8))?;
        expect(
            8,
            sessions.len() == 1 && sessions[0].session_id.as_str() == "main",
            "list_sessions included a subagent transcript",
        )?;
    }

    if has_summaries {
        // 14. list_session_summaries returns persisted fold output that round-trips through the fold again
        let store = make_store().await;
        let summarized = key("proj", "summ-sess")?;
        store
            .append(
                &summarized,
                &entries(vec![
                    json!({"timestamp": "2024-01-01T00:00:00.000Z", "customTitle": "first"}),
                    json!({"timestamp": "2024-01-01T00:00:01.000Z"}),
                ])?,
            )
            .await
            .map_err(store_error(14))?;
        store
            .append(
                &summarized,
                &entries(vec![
                    json!({"timestamp": "2024-01-01T00:00:02.000Z", "customTitle": "second"}),
                ])?,
            )
            .await
            .map_err(store_error(14))?;
        store
            .append(
                &key("other", "elsewhere")?,
                &entries(vec![json!({"timestamp": "2024-01-01T00:00:00.000Z"})])?,
            )
            .await
            .map_err(store_error(14))?;
        let summaries = store
            .list_session_summaries(&project("proj")?)
            .await
            .map_err(store_error(14))?;
        expect(
            14,
            summaries.len() == 1 && summaries[0].session_id.as_str() == "summ-sess",
            "summaries are not scoped to the project",
        )?;
        let summary = &summaries[0];
        expect(
            14,
            summary.modified_at_ms > 1_000_000_000_000,
            "summary modified time is not epoch milliseconds",
        )?;
        if has_list_sessions {
            let listed = store.list_sessions(&project("proj")?).await.map_err(store_error(14))?;
            let listed = listed
                .iter()
                .find(|session| session.session_id.as_str() == "summ-sess")
                .map(|session| session.modified_at_ms);
            expect(
                14,
                listed.is_some_and(|listed| summary.modified_at_ms >= listed),
                "summary modified time is older than list_sessions",
            )?;
        }
        let refolded = fold_session_summary(
            options.dialect,
            Some(summary),
            &summarized,
            &entries(vec![json!({"timestamp": "2024-01-01T00:00:03.000Z"})])?,
            None,
        );
        expect(
            14,
            refolded.session_id.as_str() == "summ-sess" && refolded.modified_at_ms == summary.modified_at_ms,
            "refolding changed the identity or modified time",
        )?;
        store
            .append(
                &sub(&summarized, "subagents/agent-1")?,
                &entries(vec![
                    json!({"timestamp": "2024-01-01T00:00:09.000Z", "customTitle": "subagent"}),
                ])?,
            )
            .await
            .map_err(store_error(14))?;
        let after = store
            .list_session_summaries(&project("proj")?)
            .await
            .map_err(store_error(14))?;
        expect(
            14,
            after.len() == 1 && after[0].data == summary.data,
            "a subagent append changed the main summary",
        )?;
        expect(
            14,
            store
                .list_session_summaries(&project("never-appended-project")?)
                .await
                .map_err(store_error(14))?
                .is_empty(),
            "unknown project listed summaries",
        )?;
        if has_delete {
            store.delete(&summarized).await.map_err(store_error(14))?;
            expect(
                14,
                store
                    .list_session_summaries(&project("proj")?)
                    .await
                    .map_err(store_error(14))?
                    .is_empty(),
                "delete left a summary behind",
            )?;
        }
    }

    if has_delete {
        // 9. delete of a main key removes it and tolerates unknown keys
        let store = make_store().await;
        store
            .delete(&key("proj", "never-written")?)
            .await
            .map_err(store_error(9))?;
        store
            .append(&main, &entries(vec![json!({"n": 1})])?)
            .await
            .map_err(store_error(9))?;
        store.delete(&main).await.map_err(store_error(9))?;
        expect(
            9,
            store.load(&main).await.map_err(store_error(9))?.is_none(),
            "deleted transcript still loads",
        )?;

        // 10. delete of a main key cascades to its subkeys only
        let store = make_store().await;
        let (sub1, sub2) = (sub(&main, "subagents/agent-1")?, sub(&main, "subagents/agent-2")?);
        let (other, other_project) = (key("proj", "sess2")?, key("other-proj", "sess")?);
        for target in [&main, &sub1, &sub2, &other, &other_project] {
            store
                .append(target, &entries(vec![json!({"n": 1})])?)
                .await
                .map_err(store_error(10))?;
        }
        store.delete(&main).await.map_err(store_error(10))?;
        for gone in [&main, &sub1, &sub2] {
            expect(
                10,
                store.load(gone).await.map_err(store_error(10))?.is_none(),
                "delete did not cascade",
            )?;
        }
        for kept in [&other, &other_project] {
            expect(
                10,
                store
                    .load(kept)
                    .await
                    .map_err(store_error(10))?
                    .is_some_and(|loaded| loaded.len() == 1),
                "delete removed an unrelated session",
            )?;
        }
        if has_subkeys {
            expect(
                10,
                store.list_subkeys(&main).await.map_err(store_error(10))?.is_empty(),
                "subkeys survived delete",
            )?;
        }
        if has_list_sessions {
            let listed = store.list_sessions(main.project_key()).await.map_err(store_error(10))?;
            expect(
                10,
                listed.iter().all(|session| session.session_id != *main.session_id()),
                "deleted session is still listed",
            )?;
        }

        // 11. delete of a subpath key removes only that subkey
        let store = make_store().await;
        for target in [&main, &sub1, &sub2] {
            store
                .append(target, &entries(vec![json!({"n": 1})])?)
                .await
                .map_err(store_error(11))?;
        }
        store.delete(&sub1).await.map_err(store_error(11))?;
        expect(
            11,
            store.load(&sub1).await.map_err(store_error(11))?.is_none(),
            "deleted subpath still loads",
        )?;
        for kept in [&sub2, &main] {
            expect(
                11,
                store
                    .load(kept)
                    .await
                    .map_err(store_error(11))?
                    .is_some_and(|loaded| loaded.len() == 1),
                "subpath delete removed another transcript",
            )?;
        }
        if has_subkeys {
            expect(
                11,
                store.list_subkeys(&main).await.map_err(store_error(11))? == ["subagents/agent-2"],
                "subpath delete left the wrong subkeys",
            )?;
        }
    }

    if has_subkeys {
        // 12. list_subkeys returns the subpaths of one session
        let store = make_store().await;
        for target in [
            main.clone(),
            sub(&main, "subagents/agent-1")?,
            sub(&main, "subagents/agent-2")?,
            sub(&key("proj", "other-sess")?, "subagents/agent-x")?,
        ] {
            store
                .append(&target, &entries(vec![json!({"n": 1})])?)
                .await
                .map_err(store_error(12))?;
        }
        let mut subkeys = store.list_subkeys(&main).await.map_err(store_error(12))?;
        subkeys.sort();
        expect(
            12,
            subkeys == ["subagents/agent-1", "subagents/agent-2"],
            "list_subkeys returned the wrong subpaths",
        )?;

        // 13. list_subkeys excludes the main transcript
        let store = make_store().await;
        store
            .append(&main, &entries(vec![json!({"n": 1})])?)
            .await
            .map_err(store_error(13))?;
        expect(
            13,
            store.list_subkeys(&main).await.map_err(store_error(13))?.is_empty(),
            "main transcript was listed as a subkey",
        )?;
        expect(
            13,
            store
                .list_subkeys(&key("proj", "never-appended")?)
                .await
                .map_err(store_error(13))?
                .is_empty(),
            "unknown session listed subkeys",
        )?;
    }
    Ok(())
}
