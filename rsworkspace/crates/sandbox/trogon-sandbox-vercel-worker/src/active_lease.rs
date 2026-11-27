use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::Route;

/// A sandbox this worker holds for a lease.
pub(crate) struct ActiveLease {
    pub(crate) name: String,
    pub(crate) session_id: String,
    pub(crate) routes: BTreeMap<u16, String>,
    pub(crate) persistent: bool,
    pub(crate) region: Option<String>,
    /// Held across a snapshot and its resume, so a concurrent op waits for the new session instead of reaching the
    /// one Vercel just ended.
    pub(crate) gate: Arc<Mutex<()>>,
    /// The session ended (a snapshot stopped it) and no new one is adopted yet.
    pub(crate) stopped: bool,
}

pub(crate) fn route_map(routes: &[Route]) -> BTreeMap<u16, String> {
    routes.iter().map(|route| (route.port, route.url.clone())).collect()
}
