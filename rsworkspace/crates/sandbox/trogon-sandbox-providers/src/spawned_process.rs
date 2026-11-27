use std::sync::Arc;

use tokio::process::Child;
use tokio::sync::Mutex;

/// A running agent process and the identifier its handle reports.
pub struct SpawnedProcess {
    pub id: String,
    pub child: Arc<Mutex<Option<Child>>>,
}
