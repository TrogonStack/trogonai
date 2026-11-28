use std::fmt;

/// The per-lease password a Docker workload presents to its sidecar's explicit proxy listener. Random for every
/// sandbox and never printed.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct EgressProxyToken(String);

impl EgressProxyToken {
    pub(crate) fn generate() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for EgressProxyToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EgressProxyToken(..)")
    }
}

#[cfg(test)]
mod tests;
