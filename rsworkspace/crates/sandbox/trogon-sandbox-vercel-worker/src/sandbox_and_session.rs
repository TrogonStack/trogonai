use serde::Deserialize;

use crate::{Route, SandboxRecord, SessionRecord};

/// What Vercel returns when a sandbox is created, fetched or resumed.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
pub struct SandboxAndSession {
    pub sandbox: SandboxRecord,
    pub session: SessionRecord,
    #[serde(default)]
    pub routes: Vec<Route>,
}
