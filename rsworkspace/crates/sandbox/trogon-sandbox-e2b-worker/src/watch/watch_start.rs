use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{FileOpResponse, FileRecursion};
use trogon_sandbox_file_ops::FileOpError;

use super::api_failure::api_failure;
use super::buffer::Buffer;
use super::constants::{MAX_WATCHES_PER_LEASE, WATCH_ID_PREFIX};
use super::watch_entry::WatchEntry;
use super::watch_outcome::WatchOutcome;
use super::watches::Watches;
use crate::envd::EnvdTarget;

fn too_many() -> FileOpError {
    FileOpError::too_large(format!(
        "a lease may hold at most {MAX_WATCHES_PER_LEASE} watches; stop one first"
    ))
}

impl Watches {
    pub(super) async fn start(
        &self,
        lease: &str,
        target: &EnvdTarget,
        user: &str,
        path: &str,
        recursion: FileRecursion,
    ) -> WatchOutcome {
        if !path.starts_with('/') {
            return Err(FileOpError::invalid_path("a watch path must be absolute"));
        }
        if self.count(lease) >= MAX_WATCHES_PER_LEASE {
            return Err(too_many());
        }
        let envd_id = self
            .envd
            .create_watcher(target, user, path, recursion)
            .await
            .map_err(|error| api_failure(&error, path))?;
        let id = format!("{WATCH_ID_PREFIX}{}", self.ids.fetch_add(1, Ordering::Relaxed) + 1);
        let watch = Arc::new(WatchEntry {
            envd_id,
            root: path.to_owned(),
            user: user.to_owned(),
            buffer: tokio::sync::Mutex::new(Buffer::new()),
            gone: CancellationToken::new(),
        });
        {
            let mut leases = self.leases();
            let held = leases.entry(lease.to_owned()).or_default();
            if held.len() < MAX_WATCHES_PER_LEASE {
                held.insert(id.clone(), Arc::clone(&watch));
                return Ok(FileOpResponse::WatchStarted { watch_id: id });
            }
        }
        // Another start took the last slot while this one was talking to envd.
        self.remove_remote(target, &watch).await;
        Err(too_many())
    }
}
