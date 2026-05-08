use bytes::Bytes;

use crate::pattern;
use crate::store::SubscriptionStore;
use crate::subscription::WebhookSubscription;

pub const KV_BUCKET: &str = "WEBHOOK_SUBSCRIPTIONS";

#[derive(Debug)]
pub enum RegistryError {
    Serialize(serde_json::Error),
    Store(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::Serialize(e) => write!(f, "serialization error: {e}"),
            RegistryError::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for RegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RegistryError::Serialize(e) => Some(e),
            RegistryError::Store(_) => None,
        }
    }
}

#[derive(Clone)]
pub struct WebhookRegistry<S: SubscriptionStore> {
    store: S,
}

impl<S: SubscriptionStore> WebhookRegistry<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }

    pub async fn register(&self, sub: &WebhookSubscription) -> Result<(), RegistryError> {
        let payload = serde_json::to_vec(sub).map_err(RegistryError::Serialize)?;
        self.store
            .put(&sub.id, Bytes::from(payload))
            .await
            .map_err(|e| RegistryError::Store(e.to_string()))?;
        Ok(())
    }

    pub async fn deregister(&self, id: &str) -> Result<(), RegistryError> {
        self.store
            .delete(id)
            .await
            .map_err(|e| RegistryError::Store(e.to_string()))?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<WebhookSubscription>, RegistryError> {
        let keys = self
            .store
            .keys()
            .await
            .map_err(|e| RegistryError::Store(e.to_string()))?;

        let mut subs = Vec::new();
        for key in keys {
            if let Ok(Some(bytes)) = self.store.get(&key).await {
                if let Ok(sub) = serde_json::from_slice::<WebhookSubscription>(&bytes) {
                    subs.push(sub);
                }
            }
        }
        Ok(subs)
    }

    pub async fn matching(&self, subject: &str) -> Vec<WebhookSubscription> {
        self.list()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|s| pattern::matches(&s.subject_pattern, subject))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::mock::MockSubscriptionStore;

    fn make_sub(id: &str, pattern: &str, url: &str) -> WebhookSubscription {
        WebhookSubscription {
            id: id.to_string(),
            subject_pattern: pattern.to_string(),
            url: url.to_string(),
            secret: None,
        }
    }

    #[tokio::test]
    async fn register_and_list() {
        let registry = WebhookRegistry::new(MockSubscriptionStore::new());
        let sub = make_sub("sub-1", "transcripts.>", "https://example.com/hook");

        registry.register(&sub).await.unwrap();

        let list = registry.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "sub-1");
    }

    #[tokio::test]
    async fn deregister_removes_entry() {
        let registry = WebhookRegistry::new(MockSubscriptionStore::new());
        let sub = make_sub("sub-2", "transcripts.>", "https://example.com/hook");

        registry.register(&sub).await.unwrap();
        registry.deregister("sub-2").await.unwrap();

        let list = registry.list().await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn matching_returns_only_matching_subscriptions() {
        let registry = WebhookRegistry::new(MockSubscriptionStore::new());
        registry
            .register(&make_sub("sub-a", "transcripts.>", "https://a.com"))
            .await
            .unwrap();
        registry
            .register(&make_sub("sub-b", "github.>", "https://b.com"))
            .await
            .unwrap();

        let matched = registry.matching("transcripts.pr.owner.repo.123.sess-abc").await;
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, "sub-a");
    }

    #[tokio::test]
    async fn matching_empty_when_no_subscriptions_match() {
        let registry = WebhookRegistry::new(MockSubscriptionStore::new());
        registry
            .register(&make_sub("sub-c", "github.>", "https://c.com"))
            .await
            .unwrap();

        let matched = registry.matching("transcripts.pr.owner.repo.123.sess-abc").await;
        assert!(matched.is_empty());
    }

    #[tokio::test]
    async fn multiple_matching_subscriptions() {
        let registry = WebhookRegistry::new(MockSubscriptionStore::new());
        registry
            .register(&make_sub("sub-d", "transcripts.>", "https://d.com"))
            .await
            .unwrap();
        registry
            .register(&make_sub("sub-e", ">", "https://e.com"))
            .await
            .unwrap();

        let matched = registry.matching("transcripts.pr.x.y.z").await;
        assert_eq!(matched.len(), 2);
    }

    #[tokio::test]
    async fn deregister_nonexistent_id_returns_ok() {
        let registry = WebhookRegistry::new(MockSubscriptionStore::new());
        registry.deregister("does-not-exist").await.unwrap();
    }

    #[tokio::test]
    async fn matching_returns_empty_when_store_fails() {
        #[derive(Clone)]
        struct FailingStore;

        #[derive(Debug)]
        struct StoreErr;
        impl std::fmt::Display for StoreErr {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "store unavailable")
            }
        }
        impl std::error::Error for StoreErr {}

        impl crate::store::SubscriptionStore for FailingStore {
            type PutError = StoreErr;
            type GetError = StoreErr;
            type DeleteError = StoreErr;
            type KeysError = StoreErr;
            async fn put(&self, _: &str, _: bytes::Bytes) -> Result<u64, StoreErr> {
                Err(StoreErr)
            }
            async fn get(&self, _: &str) -> Result<Option<bytes::Bytes>, StoreErr> {
                Err(StoreErr)
            }
            async fn delete(&self, _: &str) -> Result<(), StoreErr> {
                Err(StoreErr)
            }
            async fn keys(&self) -> Result<Vec<String>, StoreErr> {
                Err(StoreErr)
            }
        }

        let registry = WebhookRegistry::new(FailingStore);
        let matched = registry.matching("transcripts.anything").await;
        assert!(matched.is_empty(), "store failure should yield empty, not panic");
    }
}
