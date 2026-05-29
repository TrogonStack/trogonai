//! Per-evaluation host state for CEL builtins (cache, audit journal, rate windows).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::errors::{CelBuiltinsError, HostFailure};
use super::spicedb::SpicedbHostBackend;

thread_local! {
    static HOST_EVAL: RefCell<Option<HostEvalContext>> = const { RefCell::new(None) };
}

const CACHE_MAX_KEY_LEN: usize = 256;
const CACHE_MAX_VALUE_BYTES: usize = 65_536;
const CACHE_TTL_MIN_SECS: u64 = 1;
const CACHE_TTL_MAX_SECS: u64 = 86_400;

#[derive(Clone, Debug)]
struct CacheEntry {
    value: serde_json::Value,
    expires_at: Instant,
}

#[derive(Debug, Default)]
struct PolicyCacheInner {
    entries: HashMap<String, CacheEntry>,
}

impl PolicyCacheInner {
    fn scoped_key(tenant_id: &str, bundle_revision: &str, key: &str) -> String {
        format!("{tenant_id}/{bundle_revision}/{key}")
    }

    fn get(&mut self, scoped: &str) -> Option<serde_json::Value> {
        let entry = self.entries.get(scoped)?;
        if entry.expires_at <= Instant::now() {
            self.entries.remove(scoped);
            return None;
        }
        Some(entry.value.clone())
    }

    fn set(&mut self, scoped: String, value: serde_json::Value, ttl: Duration) -> bool {
        self.entries.insert(
            scoped,
            CacheEntry {
                value,
                expires_at: Instant::now() + ttl,
            },
        );
        true
    }
}

#[derive(Clone, Debug, Default)]
pub struct PolicyCache(Arc<Mutex<PolicyCacheInner>>);

impl PolicyCache {
    pub fn get(&self, tenant_id: &str, bundle_revision: &str, key: &str) -> Option<serde_json::Value> {
        let scoped = PolicyCacheInner::scoped_key(tenant_id, bundle_revision, key);
        self.0.lock().expect("policy cache lock").get(&scoped)
    }

    pub fn set(
        &self,
        tenant_id: &str,
        bundle_revision: &str,
        key: &str,
        value: serde_json::Value,
        ttl: Duration,
    ) -> bool {
        let scoped = PolicyCacheInner::scoped_key(tenant_id, bundle_revision, key);
        self.0
            .lock()
            .expect("policy cache lock")
            .set(scoped, value, ttl)
    }
}

#[derive(Debug)]
struct SlidingWindow {
    window: Duration,
    budget: u32,
    events: VecDeque<Instant>,
}

impl SlidingWindow {
    fn new(window: Duration, budget: u32) -> Self {
        Self {
            window,
            budget,
            events: VecDeque::new(),
        }
    }

    fn try_acquire(&mut self, now: Instant) -> bool {
        while self
            .events
            .front()
            .is_some_and(|ts| now.duration_since(*ts) >= self.window)
        {
            self.events.pop_front();
        }
        if self.events.len() >= self.budget as usize {
            return false;
        }
        self.events.push_back(now);
        true
    }
}

#[derive(Debug, Default)]
struct PolicyRateInner {
    local: HashMap<String, SlidingWindow>,
}

#[derive(Clone, Debug, Default)]
pub struct PolicyRateLimiter(Arc<Mutex<PolicyRateInner>>);

impl PolicyRateLimiter {
    pub fn acquire_local(&self, key: &str, budget: u32, window: Duration) -> bool {
        let now = Instant::now();
        let mut inner = self.0.lock().expect("policy rate lock");
        let bucket = inner
            .local
            .entry(key.to_string())
            .or_insert_with(|| SlidingWindow::new(window, budget));
        bucket.window = window;
        bucket.budget = budget;
        bucket.try_acquire(now)
    }
}

#[derive(Debug, Default)]
pub struct AuditJournal {
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone)]
pub struct HostEvalContext {
    pub tenant_id: String,
    pub bundle_revision: String,
    pub session_id: Option<String>,
    pub server_id: Option<String>,
    pub cache: PolicyCache,
    pub rate: PolicyRateLimiter,
    pub audit: Arc<Mutex<AuditJournal>>,
    pub spicedb: Option<Arc<dyn SpicedbHostBackend>>,
    clock_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl std::fmt::Debug for HostEvalContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostEvalContext")
            .field("tenant_id", &self.tenant_id)
            .field("bundle_revision", &self.bundle_revision)
            .field("session_id", &self.session_id)
            .field("server_id", &self.server_id)
            .finish_non_exhaustive()
    }
}

impl Default for HostEvalContext {
    fn default() -> Self {
        Self::for_tests()
    }
}

impl HostEvalContext {
    #[must_use]
    pub fn for_tests() -> Self {
        Self {
            tenant_id: "test-tenant".into(),
            bundle_revision: "rev-0".into(),
            session_id: None,
            server_id: None,
            cache: PolicyCache::default(),
            rate: PolicyRateLimiter::default(),
            audit: Arc::new(Mutex::new(AuditJournal::default())),
            spicedb: None,
            clock_ms: Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as i64)
            }),
        }
    }

    #[must_use]
    pub fn with_spicedb(mut self, backend: Arc<dyn SpicedbHostBackend>) -> Self {
        self.spicedb = Some(backend);
        self
    }

    #[must_use]
    pub fn with_clock_ms(mut self, clock_ms: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.clock_ms = clock_ms;
        self
    }

    pub fn now_unix_ms(&self) -> i64 {
        (self.clock_ms)()
    }

    pub fn cache_get(&self, key: &str) -> Result<Option<serde_json::Value>, CelBuiltinsError> {
        if key.len() > CACHE_MAX_KEY_LEN {
            return Err(CelBuiltinsError::policy_fault(
                super::cache::GET_NAME,
                "cache key exceeds 256 bytes",
            ));
        }
        Ok(self.cache.get(&self.tenant_id, &self.bundle_revision, key))
    }

    pub fn cache_set(
        &self,
        key: &str,
        value: serde_json::Value,
        ttl_secs: u64,
    ) -> Result<bool, CelBuiltinsError> {
        if key.len() > CACHE_MAX_KEY_LEN {
            return Err(CelBuiltinsError::policy_fault(
                super::cache::SET_NAME,
                "cache key exceeds 256 bytes",
            ));
        }
        let ttl_secs = ttl_secs.clamp(CACHE_TTL_MIN_SECS, CACHE_TTL_MAX_SECS);
        let serialized = serde_json::to_vec(&value).map_err(|err| {
            CelBuiltinsError::policy_fault(super::cache::SET_NAME, err.to_string())
        })?;
        if serialized.len() > CACHE_MAX_VALUE_BYTES {
            return Ok(false);
        }
        Ok(self.cache.set(
            &self.tenant_id,
            &self.bundle_revision,
            key,
            value,
            Duration::from_secs(ttl_secs),
        ))
    }

    pub fn rate_acquire(
        &self,
        scope: &str,
        key: &str,
        budget: u32,
        window: Duration,
    ) -> Result<bool, CelBuiltinsError> {
        if budget == 0 {
            return Err(CelBuiltinsError::policy_fault(
                super::rate::ACQUIRE_NAME,
                "budget must be > 0",
            ));
        }
        match scope {
            "local" => Ok(self.rate.acquire_local(key, budget, window)),
            "cluster" => Err(CelBuiltinsError::authz_unreachable(
                super::rate::ACQUIRE_NAME,
                "cluster rate KV unreachable",
            )),
            other => Err(CelBuiltinsError::policy_fault(
                super::rate::ACQUIRE_NAME,
                format!("unknown scope {other:?}; expected \"local\" or \"cluster\""),
            )),
        }
    }

    pub fn audit_emit(
        &self,
        fields: BTreeMap<String, serde_json::Value>,
    ) -> Result<bool, CelBuiltinsError> {
        for key in fields.keys() {
            if super::audit::RESERVED_AUDIT_KEYS.contains(&key.as_str()) {
                return Err(CelBuiltinsError::policy_fault(
                    super::audit::EMIT_NAME,
                    format!("reserved audit key {key:?}"),
                ));
            }
        }
        let mut journal = self.audit.lock().expect("audit journal lock");
        for (key, value) in fields {
            journal.extra.insert(key, value);
        }
        Ok(true)
    }

    pub fn spicedb_check(
        &self,
        subject: &str,
        permission: &str,
        resource: &str,
    ) -> Result<bool, CelBuiltinsError> {
        let Some(backend) = self.spicedb.as_ref() else {
            return Err(CelBuiltinsError::authz_unreachable(
                super::spicedb::BUILTIN_NAME,
                "spicedb host backend not configured",
            ));
        };
        match backend.check(subject, permission, resource) {
            Ok(allowed) => Ok(allowed),
            Err(failure) => match failure {
                HostFailure::Transient => Err(CelBuiltinsError::authz_unreachable(
                    super::spicedb::BUILTIN_NAME,
                    "PDP unreachable",
                )),
                HostFailure::Permanent => Err(CelBuiltinsError::policy_fault(
                    super::spicedb::BUILTIN_NAME,
                    "malformed SpiceDB reference",
                )),
                HostFailure::NotApplicable => Ok(false),
            },
        }
    }
}

pub fn with_host_eval<R>(ctx: &HostEvalContext, f: impl FnOnce() -> R) -> R {
    HOST_EVAL.with(|cell| {
        *cell.borrow_mut() = Some(ctx.clone());
        let out = f();
        *cell.borrow_mut() = None;
        out
    })
}

pub(crate) fn current_host_eval() -> Option<HostEvalContext> {
    HOST_EVAL.with(|cell| cell.borrow().clone())
}
