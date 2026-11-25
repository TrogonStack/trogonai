//! Which patterns `re._compile` (CPython 3.12) finds compiled: `_cache2`, first in first out with room for
//! `_MAXCACHE2` patterns, and `_cache`, least recently used with room for `_MAXCACHE`. `jsonschema` compiles every
//! pattern as a `str` with no flags, so a pattern alone keys both caches.
//!
//! Both caches are module globals, so every call in a Python process shares them, and a process starts with the
//! patterns its imports compiled. [`process_cache`] is that state for the whole Rust process, starting from
//! [`ReCache::seeded`].

use crate::constants::{MAX_CACHE, MAX_CACHE2, PROCESS};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::{Arc, LazyLock, Mutex},
};

use crate::python_string::PythonString;

/// The caches of the Rust process, shared by every server that does not hold its own.
static PROCESS_CACHE: LazyLock<Arc<Mutex<ReCache>>> = LazyLock::new(|| Arc::new(Mutex::new(ReCache::seeded())));

/// The caches every call of the Rust process shares, as every call of a Python process shares `re`'s.
pub(crate) fn process_cache() -> Arc<Mutex<ReCache>> {
    Arc::clone(&PROCESS_CACHE)
}

/// A key of `_cache` and `_cache2`: a pattern `jsonschema` can look up, or the position in [`PROCESS`] of a key it
/// never looks up.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Key {
    Pattern(PythonString),
    Other(usize),
}

impl From<&str> for Key {
    fn from(pattern: &str) -> Self {
        Self::Pattern(pattern.into())
    }
}

impl From<&PythonString> for Key {
    fn from(pattern: &PythonString) -> Self {
        Self::Pattern(pattern.clone())
    }
}

/// The two caches of compiled patterns.
#[derive(Debug, Default)]
pub(crate) struct ReCache {
    /// `_cache`: when each key was last stored or found there.
    used: HashMap<Key, u64>,
    /// `_cache` in use order, least recently used first.
    by_use: BTreeMap<u64, Key>,
    /// The next use stamp.
    clock: u64,
    /// `_cache2`.
    recent: HashSet<Key>,
    /// `_cache2` in insertion order, oldest first.
    by_insertion: VecDeque<Key>,
    /// The `jsonschema` instructions the process already executed once, a bit each as the validator numbers them:
    /// CPython specializes an instruction after its first execution, which alone pays C recursion checks. The bits
    /// travel with the `re` caches, as both are process state every call reads and leaves behind.
    pub(crate) executed: u16,
}

impl ReCache {
    /// The caches of a Python process at its first tool call: the keys of [`PROCESS`], stored in order.
    pub(crate) fn seeded() -> Self {
        let mut cache = Self::default();
        for (index, pattern) in PROCESS.into_iter().enumerate() {
            cache.store(pattern.map_or(Key::Other(index), Key::from));
        }
        cache
    }

    /// Whether `re._compile` returns `pattern` without compiling it.
    pub(crate) fn contains(&self, pattern: impl Into<Key>) -> bool {
        let key = pattern.into();
        self.recent.contains(&key) || self.used.contains_key(&key)
    }

    /// What `re._compile` does with a pattern it finds: one in `_cache2` stays as it is, and one only in `_cache`
    /// moves to the end of `_cache` and joins `_cache2`.
    pub(crate) fn found(&mut self, pattern: impl Into<Key>) {
        let key = pattern.into();
        if self.recent.contains(&key) {
            return;
        }
        if let Some(stamp) = self.used.remove(&key) {
            self.by_use.remove(&stamp);
            self.use_now(key.clone());
            self.add_recent(key);
        }
    }

    /// What `re._compile` does with a pattern it compiled.
    pub(crate) fn compiled(&mut self, pattern: impl Into<Key>) {
        self.store(pattern.into());
    }

    /// Stores a key in neither cache: the least recently used key leaves a full `_cache` and the oldest leaves a
    /// full `_cache2` before the key joins both.
    fn store(&mut self, key: Key) {
        if self.used.len() >= MAX_CACHE
            && let Some((_, oldest)) = self.by_use.pop_first()
        {
            self.used.remove(&oldest);
        }
        self.use_now(key.clone());
        self.add_recent(key);
    }

    fn use_now(&mut self, key: Key) {
        self.clock += 1;
        self.used.insert(key.clone(), self.clock);
        self.by_use.insert(self.clock, key);
    }

    fn add_recent(&mut self, key: Key) {
        if self.recent.len() >= MAX_CACHE2
            && let Some(oldest) = self.by_insertion.pop_front()
        {
            self.recent.remove(&oldest);
        }
        self.recent.insert(key.clone());
        self.by_insertion.push_back(key);
    }
}

#[cfg(test)]
#[path = "python_re_cache_tests.rs"]
mod tests;
