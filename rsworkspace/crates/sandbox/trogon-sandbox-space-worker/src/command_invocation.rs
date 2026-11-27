use std::collections::BTreeMap;

use crate::SpaceId;

/// Everything one command needs to run.
pub(crate) struct CommandInvocation<'a> {
    pub(crate) sandbox: &'a SpaceId,
    pub(crate) argv: &'a [String],
    pub(crate) env: &'a BTreeMap<String, String>,
    pub(crate) cwd: Option<&'a str>,
    pub(crate) timeout_seconds: u32,
    pub(crate) cap: usize,
    pub(crate) key: &'a str,
}
