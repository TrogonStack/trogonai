use std::{
    collections::HashMap,
    error::Error,
    sync::{Arc, Mutex},
};

use trogon_claude_agent_sdk::{
    InMemorySessionStore, SessionEntry, SessionKey, SessionStore, SessionStoreError, SessionStoreFuture,
    SessionStoreOperation, SummaryDialect,
    testing::{ConformanceOptions, run_session_store_conformance},
};

type TestResult = Result<(), Box<dyn Error>>;

/// Implements only the required operations, so every optional contract must be skipped.
#[derive(Default)]
struct MinimalStore {
    entries: Mutex<HashMap<SessionKey, Vec<SessionEntry>>>,
}

impl SessionStore for MinimalStore {
    fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            if entries.is_empty() {
                return Ok(());
            }
            self.entries
                .lock()
                .map_err(|_| SessionStoreError::Poisoned)?
                .entry(key.clone())
                .or_default()
                .extend(entries.iter().cloned());
            Ok(())
        })
    }

    fn load<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async move {
            Ok(self
                .entries
                .lock()
                .map_err(|_| SessionStoreError::Poisoned)?
                .get(key)
                .cloned())
        })
    }
}

#[tokio::test]
async fn in_memory_store_satisfies_every_contract_with_the_typescript_summary_layout() -> TestResult {
    run_session_store_conformance(
        || async { Arc::new(InMemorySessionStore::new()) as Arc<dyn SessionStore> },
        ConformanceOptions::default(),
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn in_memory_store_satisfies_every_contract_with_the_python_summary_layout() -> TestResult {
    run_session_store_conformance(
        || async {
            Arc::new(InMemorySessionStore::new().with_summary_dialect(SummaryDialect::Python)) as Arc<dyn SessionStore>
        },
        ConformanceOptions {
            dialect: SummaryDialect::Python,
            ..ConformanceOptions::default()
        },
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_required_only_store_passes_without_optional_operations() -> TestResult {
    assert!(!MinimalStore::default().supports(SessionStoreOperation::List));
    run_session_store_conformance(
        || async { Arc::new(MinimalStore::default()) as Arc<dyn SessionStore> },
        ConformanceOptions::default(),
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn skipped_operations_are_not_exercised() -> TestResult {
    run_session_store_conformance(
        || async { Arc::new(InMemorySessionStore::new()) as Arc<dyn SessionStore> },
        ConformanceOptions {
            skip_optional: vec![
                SessionStoreOperation::List,
                SessionStoreOperation::ListSummaries,
                SessionStoreOperation::Delete,
                SessionStoreOperation::ListSubkeys,
            ],
            ..ConformanceOptions::default()
        },
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_store_that_loses_order_fails_the_ordering_contract() -> TestResult {
    struct Reversing(MinimalStore);

    impl SessionStore for Reversing {
        fn append<'a>(&'a self, key: &'a SessionKey, entries: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
            self.0.append(key, entries)
        }

        fn load<'a>(&'a self, key: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
            Box::pin(async move {
                let mut loaded = self.0.load(key).await?;
                if let Some(entries) = loaded.as_mut() {
                    entries.reverse();
                }
                Ok(loaded)
            })
        }
    }

    let failure = run_session_store_conformance(
        || async { Arc::new(Reversing(MinimalStore::default())) as Arc<dyn SessionStore> },
        ConformanceOptions::default(),
    )
    .await
    .err()
    .ok_or("a reordering store passed")?;
    assert_eq!(failure.check, 1);
    Ok(())
}
