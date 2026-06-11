//! Live sub-agent spawn tracking for the REPL `/tasks` command.
//!
//! Active NATS sub-sessions are discovered via the runner's `list_sessions`
//! response (`parentSessionId` in `_meta`). In-flight `spawn_agent` tool calls
//! that have not yet registered a child session are tracked locally.

use crate::session::SessionSummary;

/// Counts in-flight `spawn_agent` tool invocations observed this REPL session.
#[derive(Debug, Default)]
pub struct SpawnTracker {
    running: u32,
}

impl SpawnTracker {
    pub fn on_tool_call(&mut self, name: &str) {
        if is_spawn_agent(name) {
            self.running = self.running.saturating_add(1);
        }
    }

    pub fn on_tool_finished(&mut self, name: &str) {
        if is_spawn_agent(name) && self.running > 0 {
            self.running -= 1;
        }
    }

    pub fn format_tasks(
        &self,
        prefix: &str,
        parent_session_id: &str,
        sessions: &[SessionSummary],
    ) -> String {
        format_active_spawns(prefix, parent_session_id, sessions, self.running)
    }
}

/// Render `/tasks` output from runner child sessions plus any pending local spawns.
pub fn format_active_spawns(
    prefix: &str,
    parent_session_id: &str,
    sessions: &[SessionSummary],
    pending_tool_calls: u32,
) -> String {
    let children: Vec<&SessionSummary> = sessions
        .iter()
        .filter(|s| s.parent_session_id.as_deref() == Some(parent_session_id))
        .collect();

    let extra_pending = pending_tool_calls.saturating_sub(children.len() as u32);
    if children.is_empty() && extra_pending == 0 {
        return "no active tasks".to_string();
    }

    let mut out = format!("active spawns on {prefix} ({parent_session_id}):\n");
    out.push_str(&format!(
        "  {:<36}  {:<10}  {:<20}  task\n",
        "session_id", "status", "started"
    ));

    for child in children {
        let started = child.updated_at.as_deref().unwrap_or("-");
        let label = child
            .title
            .as_deref()
            .filter(|t| !t.is_empty())
            .unwrap_or(child.cwd.as_str());
        out.push_str(&format!(
            "  {:<36}  {:<10}  {:<20}  {label}\n",
            child.session_id, "running", started
        ));
    }

    for i in 0..extra_pending {
        let id = format!("pending-{}", i + 1);
        out.push_str(&format!(
            "  {:<36}  {:<10}  {:<20}  (starting…)\n",
            id, "running", "-"
        ));
    }

    out.trim_end().to_string()
}

fn is_spawn_agent(name: &str) -> bool {
    name == "spawn_agent" || name.ends_with("__spawn_agent")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_child(id: &str, parent: &str) -> SessionSummary {
        SessionSummary {
            session_id: id.to_string(),
            cwd: "/tmp/worktree".to_string(),
            title: Some("review auth module".to_string()),
            updated_at: Some("2026-06-10T12:00:00Z".to_string()),
            parent_session_id: Some(parent.to_string()),
        }
    }

    #[test]
    fn empty_shows_no_active_tasks() {
        let out = format_active_spawns("acp.claude", "parent-1", &[], 0);
        assert_eq!(out, "no active tasks");
    }

    #[test]
    fn lists_child_sessions_from_runner() {
        let sessions = vec![
            sample_child("child-a", "parent-1"),
            SessionSummary {
                session_id: "other".into(),
                cwd: "/".into(),
                title: None,
                updated_at: None,
                parent_session_id: None,
            },
        ];
        let out = format_active_spawns("acp.claude", "parent-1", &sessions, 0);
        assert!(out.contains("child-a"));
        assert!(out.contains("running"));
        assert!(out.contains("review auth module"));
        assert!(!out.contains("other"));
    }

    #[test]
    fn pending_tool_call_without_child_session_shown() {
        let out = format_active_spawns("acp.claude", "parent-1", &[], 1);
        assert!(out.contains("pending-1"));
        assert!(out.contains("(starting…)"));
    }

    #[test]
    fn tracker_counts_spawn_agent_tool_calls() {
        let mut tracker = SpawnTracker::default();
        tracker.on_tool_call("spawn_agent");
        let pending = tracker.format_tasks("acp.claude", "parent-1", &[]);
        assert!(pending.contains("pending-1"));
        tracker.on_tool_finished("spawn_agent");
        assert_eq!(tracker.format_tasks("acp.claude", "parent-1", &[]), "no active tasks");
    }

    #[test]
    fn tracker_ignores_unrelated_tools() {
        let mut tracker = SpawnTracker::default();
        tracker.on_tool_call("bash");
        tracker.on_tool_finished("read_file");
        assert_eq!(tracker.format_tasks("acp.claude", "parent-1", &[]), "no active tasks");
    }
}
