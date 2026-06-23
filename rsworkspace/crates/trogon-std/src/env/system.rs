use std::env;

use super::ReadEnv;

/// Zero-sized type — delegates to `std::env`.
pub struct SystemEnv;

impl ReadEnv for SystemEnv {
    #[inline]
    fn var(&self, key: &str) -> Result<String, env::VarError> {
        env::var(key)
    }
}

#[cfg(test)]
mod tests;
