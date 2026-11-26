use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Per-lease counters the broker keeps in its shared usage store, so every replica reads the same
/// figures.
///
/// [`LeaseCounters::add`] merges the additive fields. `paused_at`, `suspended_at`, and
/// `suspend_artifact_bytes` track the current pause or suspend and are never part of a delta.
#[derive(Debug, Clone, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct LeaseCounters {
    /// Commands the broker dispatched that a worker accepted, foreground or detached.
    #[serde(default)]
    pub commands_run: u64,
    /// Bytes of stdout and stderr returned by completed commands.
    #[serde(default)]
    pub command_output_bytes: u64,
    /// Bytes written into the sandbox through file verbs.
    #[serde(default)]
    pub file_bytes_in: u64,
    /// Bytes read out of the sandbox through file verbs.
    #[serde(default)]
    pub file_bytes_out: u64,
    /// Whole seconds spent paused in earlier pause and resume cycles.
    #[serde(default)]
    pub paused_seconds: u64,
    /// When the lease was last paused. `Some` only while it is paused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_at: Option<SystemTime>,
    /// Whole seconds spent suspended in earlier suspend and wake cycles.
    #[serde(default)]
    pub suspended_seconds: u64,
    /// When the lease was last suspended. `Some` only while it is suspended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspended_at: Option<SystemTime>,
    /// Bytes the lease's suspend artifact takes now. Absent unless suspended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspend_artifact_bytes: Option<u64>,
}

impl LeaseCounters {
    /// Adds the additive counters of `delta`, saturating instead of wrapping.
    pub fn add(&mut self, delta: &Self) {
        self.commands_run = self.commands_run.saturating_add(delta.commands_run);
        self.command_output_bytes = self.command_output_bytes.saturating_add(delta.command_output_bytes);
        self.file_bytes_in = self.file_bytes_in.saturating_add(delta.file_bytes_in);
        self.file_bytes_out = self.file_bytes_out.saturating_add(delta.file_bytes_out);
        self.paused_seconds = self.paused_seconds.saturating_add(delta.paused_seconds);
        self.suspended_seconds = self.suspended_seconds.saturating_add(delta.suspended_seconds);
    }
}
