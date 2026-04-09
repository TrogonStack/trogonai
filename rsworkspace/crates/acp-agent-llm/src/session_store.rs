use crate::session::Session;
use std::cell::RefCell;
use std::collections::HashMap;

/// In-memory storage for active sessions.
/// Uses `RefCell` (not `Arc<Mutex>`) because the agent runs on a
/// single-threaded `LocalSet` — matching the `?Send` ACP Agent trait.
pub struct SessionStore {
    sessions: RefCell<HashMap<String, Session>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: RefCell::new(HashMap::new()),
        }
    }

    pub fn insert(&self, id: String, session: Session) {
        self.sessions.borrow_mut().insert(id, session);
    }

    pub fn remove(&self, id: &str) -> Option<Session> {
        self.sessions.borrow_mut().remove(id)
    }

    /// Access a session mutably by ID. Returns `None` if not found.
    pub fn with<F, R>(&self, id: &str, f: F) -> Option<R>
    where
        F: FnOnce(&mut Session) -> R,
    {
        self.sessions.borrow_mut().get_mut(id).map(f)
    }

    pub fn list_ids(&self) -> Vec<String> {
        self.sessions.borrow().keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_id::ModelId;
    use crate::provider_name::ProviderName;

    fn test_session() -> Session {
        Session::new(
            ProviderName::new("anthropic").unwrap(),
            ModelId::new("test").unwrap(),
        )
    }

    #[test]
    fn insert_and_list() {
        let store = SessionStore::new();
        store.insert("s1".to_string(), test_session());
        store.insert("s2".to_string(), test_session());
        assert_eq!(store.list_ids().len(), 2);
    }

    #[test]
    fn remove_returns_session() {
        let store = SessionStore::new();
        store.insert("s1".to_string(), test_session());
        assert!(store.remove("s1").is_some());
        assert!(store.remove("s1").is_none());
    }

    #[test]
    fn with_accesses_session() {
        let store = SessionStore::new();
        store.insert("s1".to_string(), test_session());
        let provider = store.with("s1", |s| s.provider_name().as_str().to_string());
        assert_eq!(provider, Some("anthropic".to_string()));
    }

    #[test]
    fn with_returns_none_for_missing() {
        let store = SessionStore::new();
        let result: Option<()> = store.with("missing", |_| ());
        assert!(result.is_none());
    }
}
