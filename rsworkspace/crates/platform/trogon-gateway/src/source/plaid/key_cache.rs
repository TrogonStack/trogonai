use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::{Mutex, RwLock};

use super::PlaidKeyId;
use super::constants::{KEY_FETCH_THROTTLE_INTERVAL, KEY_REFRESH_INTERVAL};
use super::key_fetcher::{PlaidKeyFetchError, PlaidKeyFetcher};
use super::verification_key::PlaidVerificationKey;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlaidKeyResolveError {
    #[error("verification key {0} is expired")]
    Expired(PlaidKeyId),
    #[error("fetching verification key {0} was throttled")]
    Throttled(PlaidKeyId),
    #[error(transparent)]
    Fetch(#[from] PlaidKeyFetchError),
}

struct CachedKey {
    key: Arc<PlaidVerificationKey>,
    fetched_at: SystemTime,
    refresh_failed_at: Option<SystemTime>,
}

impl CachedKey {
    fn is_stale_at(&self, now: SystemTime) -> bool {
        now.duration_since(self.fetched_at)
            .is_ok_and(|age| age >= KEY_REFRESH_INTERVAL)
    }
}

enum Lookup {
    Fresh(Arc<PlaidVerificationKey>),
    Stale { refresh_failed_at: Option<SystemTime> },
    Unknown,
}

#[derive(Clone)]
pub struct PlaidKeyCache<F> {
    fetcher: F,
    keys: Arc<RwLock<HashMap<PlaidKeyId, CachedKey>>>,
    fetch_gate: Arc<Mutex<Option<SystemTime>>>,
}

impl<F: PlaidKeyFetcher> PlaidKeyCache<F> {
    pub fn new(fetcher: F) -> Self {
        Self {
            fetcher,
            keys: Arc::default(),
            fetch_gate: Arc::default(),
        }
    }

    pub async fn resolve(
        &self,
        key_id: &PlaidKeyId,
        now: SystemTime,
    ) -> Result<Arc<PlaidVerificationKey>, PlaidKeyResolveError> {
        if let Lookup::Fresh(key) = self.lookup(key_id, now).await {
            return Self::unexpired(key, now);
        }

        let mut last_unknown_fetch = self.fetch_gate.lock().await;
        match self.lookup(key_id, now).await {
            Lookup::Fresh(key) => return Self::unexpired(key, now),
            Lookup::Stale { refresh_failed_at } => {
                if Self::throttled(refresh_failed_at, now) {
                    return Err(PlaidKeyResolveError::Throttled(key_id.clone()));
                }
            }
            Lookup::Unknown => {
                if Self::throttled(*last_unknown_fetch, now) {
                    return Err(PlaidKeyResolveError::Throttled(key_id.clone()));
                }
                *last_unknown_fetch = Some(now);
            }
        }

        let key = match self.fetcher.fetch(key_id).await {
            Ok(key) => Arc::new(key),
            Err(err) => {
                if let Some(cached) = self.keys.write().await.get_mut(key_id) {
                    cached.refresh_failed_at = Some(now);
                }
                return Err(err.into());
            }
        };
        self.keys.write().await.insert(
            key_id.clone(),
            CachedKey {
                key: Arc::clone(&key),
                fetched_at: now,
                refresh_failed_at: None,
            },
        );
        Self::unexpired(key, now)
    }

    async fn lookup(&self, key_id: &PlaidKeyId, now: SystemTime) -> Lookup {
        match self.keys.read().await.get(key_id) {
            Some(cached) if cached.is_stale_at(now) => Lookup::Stale {
                refresh_failed_at: cached.refresh_failed_at,
            },
            Some(cached) => Lookup::Fresh(Arc::clone(&cached.key)),
            None => Lookup::Unknown,
        }
    }

    fn throttled(last_fetch: Option<SystemTime>, now: SystemTime) -> bool {
        last_fetch.is_some_and(|last| {
            now.duration_since(last)
                .is_ok_and(|elapsed| elapsed < KEY_FETCH_THROTTLE_INTERVAL)
        })
    }

    fn unexpired(
        key: Arc<PlaidVerificationKey>,
        now: SystemTime,
    ) -> Result<Arc<PlaidVerificationKey>, PlaidKeyResolveError> {
        if key.is_expired_at(now) {
            return Err(PlaidKeyResolveError::Expired(key.key_id().clone()));
        }
        Ok(key)
    }
}

#[cfg(test)]
mod tests;
