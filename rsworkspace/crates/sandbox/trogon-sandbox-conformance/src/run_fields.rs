use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;
use trogon_sandbox_contract::LeaseId;

/// Extra fields of a `run_command` request beyond the lease and argv.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct RunFields<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detached: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) env: Option<BTreeMap<&'a str, &'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_output_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) process: Option<ProcessSpec<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) timeout_seconds: Option<u64>,
}

impl<'a> RunFields<'a> {
    pub(crate) fn timeout(seconds: u64) -> Self {
        Self {
            timeout_seconds: Some(seconds),
            ..Self::default()
        }
    }

    pub(crate) fn detached(detached: bool) -> Self {
        Self {
            detached: Some(detached),
            ..Self::default()
        }
    }

    pub(crate) fn cwd(cwd: &'a str) -> Self {
        Self {
            cwd: Some(cwd),
            ..Self::default()
        }
    }

    pub(crate) fn env(name: &'a str, value: &'a str) -> Self {
        Self {
            env: Some(BTreeMap::from([(name, value)])),
            ..Self::default()
        }
    }
}

impl fmt::Display for RunFields<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let json = serde_json::to_string(self).map_err(|_| fmt::Error)?;
        f.write_str(&json)
    }
}

/// The `process` field of a detached `run_command` request.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ProcessSpec<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) readiness: Option<Readiness<'a>>,
}

/// When a started process counts as ready.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Readiness<'a> {
    OutputContains { needle: &'a str },
    TcpPort { port: u16 },
}

impl<'a> ProcessSpec<'a> {
    pub(crate) fn named(name: &'a str) -> Self {
        Self {
            name: Some(name),
            readiness: None,
        }
    }

    pub(crate) fn ready_when(readiness: Readiness<'a>) -> Self {
        Self {
            name: None,
            readiness: Some(readiness),
        }
    }
}

/// The full `run_command` request body: the lease, the argv and the extra fields.
#[derive(Serialize)]
pub(crate) struct RunBody<'a> {
    pub(crate) lease_id: &'a LeaseId,
    pub(crate) command: &'a [&'a str],
    #[serde(flatten)]
    pub(crate) fields: &'a RunFields<'a>,
}

#[cfg(test)]
mod tests;
