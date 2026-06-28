use std::env;
use std::ffi::OsString;

use super::ReadEnv;

/// Zero-sized type — delegates to `std::env`.
pub struct SystemEnv;

impl ReadEnv for SystemEnv {
    #[inline]
    fn var(&self, key: &str) -> Result<String, env::VarError> {
        env::var(key)
    }

    #[inline]
    fn var_os(&self, key: &str) -> Option<OsString> {
        env::var_os(key)
    }

    #[inline]
    fn vars(&self) -> Vec<(String, String)> {
        env::vars().collect()
    }

    #[inline]
    fn vars_os(&self) -> Vec<(OsString, OsString)> {
        env::vars_os().collect()
    }
}

#[cfg(test)]
mod tests;
