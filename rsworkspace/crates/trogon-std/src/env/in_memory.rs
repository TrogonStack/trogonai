#[cfg(any(test, feature = "test-support"))]
use std::cell::RefCell;
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "test-support"))]
use std::env;
#[cfg(any(test, feature = "test-support"))]
use std::ffi::OsString;

#[cfg(any(test, feature = "test-support"))]
use super::ReadEnv;

/// Won't touch the global process environment.
///
/// Uses `RefCell` for interior mutability — all methods take `&self`,
/// consistent with [`MemFs`](crate::fs::MemFs) and
/// [`MockClock`](crate::time::MockClock).
#[cfg(any(test, feature = "test-support"))]
pub struct InMemoryEnv {
    vars: RefCell<HashMap<String, String>>,
}

#[cfg(any(test, feature = "test-support"))]
impl InMemoryEnv {
    pub fn new() -> Self {
        Self {
            vars: RefCell::new(HashMap::new()),
        }
    }

    pub fn set(&self, key: impl Into<String>, value: impl Into<String>) {
        self.vars.borrow_mut().insert(key.into(), value.into());
    }

    pub fn remove(&self, key: &str) {
        self.vars.borrow_mut().remove(key);
    }

    pub fn contains(&self, key: &str) -> bool {
        self.vars.borrow().contains_key(key)
    }

    pub fn clear(&self) {
        self.vars.borrow_mut().clear();
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for InMemoryEnv {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ReadEnv for InMemoryEnv {
    fn var(&self, key: &str) -> Result<String, env::VarError> {
        self.vars.borrow().get(key).cloned().ok_or(env::VarError::NotPresent)
    }

    fn var_os(&self, key: &str) -> Option<OsString> {
        self.vars.borrow().get(key).map(OsString::from)
    }

    fn vars(&self) -> Vec<(String, String)> {
        self.vars.borrow().iter().map(|(key, value)| (key.clone(), value.clone())).collect()
    }

    fn vars_os(&self) -> Vec<(OsString, OsString)> {
        self.vars
            .borrow()
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }
}

#[cfg(test)]
mod tests;
