//! Live inotify feeds behind file watches.
//!
//! When the image has `inotifywait`, each watch gets one long-lived `exec` running
//! `inotifywait -m` over the watched tree. Its events wake a parked poll at once and are
//! handed to the tracking state machine as [`ChangeHint`]s, which recover what two scans
//! cannot tell apart: a path created and deleted between them, and a rewrite that keeps
//! the size and timestamp. Scans stay the source of truth, so a feed that dies, overflows
//! or never starts degrades to the scan path rather than to wrong answers.
//!
//! Residuals:
//! - Changes in the moment between the watch's first scan and `inotifywait` placing its
//!   watches reach the poll only as the scans' net difference.
//! - A path whose name contains a newline is split across two lines and dropped.
//! - With a recursive watch, `inotifywait` adds watches on new directories after they
//!   appear, so a file created and deleted inside a brand new directory in that gap is
//!   seen only through the scans.
//! - The kernel's own queue can overflow under a burst; the lost events are lost to the
//!   hints too, and the scans still report the net change.
//! - A watch that expires from idleness keeps its feed until the next watch request on
//!   the lease or the lease ends.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bollard::Docker;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use futures::StreamExt;
use tokio::sync::Notify;
use tokio::task::AbortHandle;
use trogon_sandbox_contract::{FileRecursion, FileWatchEventKind, FileWatchTarget};
use trogon_sandbox_file_ops::ChangeHint;

use super::constants::{FEED_SCRIPT, INOTIFY_MISSING_EXIT, MAX_PENDING_HINTS};
use super::{InotifyMissing, absolute};

/// One line of [`FEED_SCRIPT`]'s output.
#[derive(Debug, Eq, PartialEq)]
enum FeedLine {
    Pid(u32),
    Hint(ChangeHint),
}

fn parse_line(line: &str) -> Option<FeedLine> {
    if let Some(pid) = line.strip_prefix("pid ") {
        return pid.trim().parse().ok().map(FeedLine::Pid);
    }
    let (events, path) = line.split_once('|')?;
    let flags: Vec<&str> = events.split(',').collect();
    let has = |flag: &str| flags.contains(&flag);
    let kind = if has("CREATE") || has("MOVED_TO") {
        FileWatchEventKind::Created
    } else if has("DELETE") || has("MOVED_FROM") {
        FileWatchEventKind::Deleted
    } else if has("MODIFY") || has("CLOSE_WRITE") || has("ATTRIB") {
        FileWatchEventKind::Modified
    } else {
        return None;
    };
    Some(FeedLine::Hint(ChangeHint {
        path: absolute(path).ok()?,
        kind,
        target: FileWatchTarget::from(has("ISDIR")),
    }))
}

#[derive(Default)]
struct FeedState {
    hints: Vec<ChangeHint>,
    pid: Option<u32>,
    ended: bool,
}

/// One watch's feed.
pub(crate) struct Feed {
    lease: String,
    state: Mutex<FeedState>,
    changed: Notify,
    task: Mutex<Option<AbortHandle>>,
}

impl Feed {
    fn new(lease: &str) -> Self {
        Self {
            lease: lease.to_string(),
            state: Mutex::default(),
            changed: Notify::new(),
            task: Mutex::default(),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FeedState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn accept(&self, line: FeedLine) {
        match line {
            FeedLine::Pid(pid) => self.state().pid = Some(pid),
            FeedLine::Hint(hint) => {
                let mut state = self.state();
                if state.hints.len() >= MAX_PENDING_HINTS {
                    state.hints.remove(0);
                }
                state.hints.push(hint);
                drop(state);
                self.changed.notify_waiters();
            }
        }
    }

    fn end(&self) {
        self.state().ended = true;
        self.changed.notify_waiters();
    }

    #[cfg(test)]
    pub(crate) fn push_for_test(&self, path: &str, kind: FileWatchEventKind) {
        self.accept(FeedLine::Hint(ChangeHint {
            path: path.to_string(),
            kind,
            target: FileWatchTarget::File,
        }));
    }

    #[cfg(test)]
    pub(crate) fn end_for_test(&self) {
        self.end();
    }

    pub(crate) fn is_live(&self) -> bool {
        !self.state().ended
    }

    pub(crate) fn drain(&self) -> Vec<ChangeHint> {
        std::mem::take(&mut self.state().hints)
    }

    /// Waits until the feed reports a change or ends, or `timeout` passes.
    pub(crate) async fn wait(&self, timeout: Duration) {
        let notified = self.changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        {
            let state = self.state();
            if state.ended || !state.hints.is_empty() {
                return;
            }
        }
        let _ = tokio::time::timeout(timeout, notified).await;
    }

    fn abort(&self) {
        if let Some(task) = self.task.lock().unwrap_or_else(PoisonError::into_inner).take() {
            task.abort();
        }
    }
}

/// The live feeds of every lease's watches, keyed by watch id.
#[derive(Default)]
pub(crate) struct WatchFeeds(Mutex<HashMap<String, Arc<Feed>>>);

impl WatchFeeds {
    fn feeds(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Feed>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn get(&self, watch_id: &str) -> Option<Arc<Feed>> {
        self.feeds().get(watch_id).cloned()
    }

    /// Starts the feed for a new watch. An image found without `inotifywait` is recorded
    /// in `missing` and the feed ends, leaving the watch on scans.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        &self,
        docker: Docker,
        container_id: String,
        lease: &str,
        watch_id: &str,
        path: String,
        recursion: FileRecursion,
        missing: Arc<InotifyMissing>,
    ) {
        let feed = Arc::new(Feed::new(lease));
        let task = tokio::spawn({
            let feed = Arc::clone(&feed);
            let lease = lease.to_string();
            async move {
                let exit = run(&docker, &container_id, &path, recursion, &feed).await;
                if exit == Some(i64::from(INOTIFY_MISSING_EXIT)) {
                    missing.insert(&lease);
                }
                feed.end();
            }
        });
        *feed.task.lock().unwrap_or_else(PoisonError::into_inner) = Some(task.abort_handle());
        if let Some(previous) = self.feeds().insert(watch_id.to_string(), feed) {
            previous.abort();
        }
    }

    /// Stops the feeds of `lease`'s watches that are no longer in `live`, killing their
    /// `inotifywait` inside the container.
    pub(crate) fn retain(&self, docker: &Docker, container_id: &str, lease: &str, live: &[String]) {
        let stopped: Vec<Arc<Feed>> = {
            let mut feeds = self.feeds();
            let gone: Vec<String> = feeds
                .iter()
                .filter(|(id, feed)| feed.lease == lease && !live.contains(id))
                .map(|(id, _)| id.clone())
                .collect();
            gone.iter().filter_map(|id| feeds.remove(id)).collect()
        };
        for feed in stopped {
            feed.abort();
            if let Some(pid) = feed.state().pid {
                let docker = docker.clone();
                let container_id = container_id.to_string();
                tokio::spawn(async move { kill(&docker, &container_id, pid).await });
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn insert_for_test(&self, watch_id: &str, lease: &str) -> Arc<Feed> {
        let feed = Arc::new(Feed::new(lease));
        self.feeds().insert(watch_id.to_string(), Arc::clone(&feed));
        feed
    }

    /// Drops every feed of `lease`, once its container is going away with them.
    pub(crate) fn forget(&self, lease: &str) {
        self.feeds().retain(|_, feed| {
            let keep = feed.lease != lease;
            if !keep {
                feed.abort();
            }
            keep
        });
    }
}

/// Runs [`FEED_SCRIPT`] until it exits, feeding its lines to `feed`. Returns the exit code
/// when Docker reports one.
async fn run(docker: &Docker, container_id: &str, path: &str, recursion: FileRecursion, feed: &Feed) -> Option<i64> {
    let cmd = vec![
        "sh".to_string(),
        "-c".to_string(),
        FEED_SCRIPT.to_string(),
        "sh".to_string(),
        path.to_string(),
        if recursion.is_recursive() { "1" } else { "0" }.to_string(),
    ];
    let exec = docker
        .create_exec(
            container_id,
            CreateExecOptions {
                cmd: Some(cmd),
                attach_stdout: Some(true),
                attach_stderr: Some(false),
                ..Default::default()
            },
        )
        .await
        .ok()?;
    let StartExecResults::Attached { mut output, .. } =
        docker.start_exec(&exec.id, None::<StartExecOptions>).await.ok()?
    else {
        return None;
    };
    let mut buffer = Vec::new();
    while let Some(Ok(chunk)) = output.next().await {
        if let LogOutput::StdOut { message } = chunk {
            buffer.extend_from_slice(&message);
            while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=end).collect();
                if let Some(line) = parse_line(String::from_utf8_lossy(&line[..end]).as_ref()) {
                    feed.accept(line);
                }
            }
        }
    }
    // Docker can report the exec as still running for a moment after its stream closes.
    for _ in 0..20 {
        match docker.inspect_exec(&exec.id).await {
            Ok(inspect) if inspect.running == Some(false) => return inspect.exit_code,
            Ok(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(_) => return None,
        }
    }
    None
}

async fn kill(docker: &Docker, container_id: &str, pid: u32) {
    let command = vec!["kill".to_string(), pid.to_string()];
    let _ = crate::adapter::DockerWorkerProvider::exec_for_file_op(docker, container_id, command, None).await;
}

#[cfg(test)]
mod tests;
