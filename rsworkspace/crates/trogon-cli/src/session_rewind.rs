//! REPL session checkpointing and rewind over conversation history.
//!
//! Checkpoints store a `session/export` snapshot at the current turn index.
//! Rewind restores history via `session/import`, either from a named checkpoint
//! or by truncating the exported history back N user turns.

use std::error::Error;
use std::fmt;

use trogon_runner_tools::portable_session::{ParsedExport, parse_export_json};

/// A named snapshot of session history at a specific turn index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub name: String,
    pub turn_index: usize,
    pub messages_json: String,
}

/// Tracks completed REPL turns and stored checkpoints for `/checkpoint` + `/rewind`.
#[derive(Debug, Clone)]
pub struct SessionRewindState {
    turn_count: usize,
    checkpoints: Vec<Checkpoint>,
    next_auto_id: usize,
}

impl Default for SessionRewindState {
    fn default() -> Self {
        Self {
            turn_count: 0,
            checkpoints: Vec::new(),
            next_auto_id: 1,
        }
    }
}

/// How to restore history on rewind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewindResolution {
    /// Use a stored export snapshot verbatim.
    Snapshot { messages_json: String, target_turn: usize },
    /// Truncate a fresh export to the first `target_turn` user turns.
    TruncateToTurns { target_turn: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewindError {
    ListRequested,
    InvalidTurns,
    TooManyTurns { requested: usize, available: usize },
    UnknownCheckpoint(String),
    ExportParse(String),
    ExportSerialize(String),
}

impl fmt::Display for RewindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ListRequested => f.write_str("list checkpoints"),
            Self::InvalidTurns => f.write_str("turn count must be at least 1"),
            Self::TooManyTurns { requested, available } => {
                write!(f, "cannot rewind {requested} turns — only {available} completed")
            }
            Self::UnknownCheckpoint(name) => write!(f, "unknown checkpoint: {name}"),
            Self::ExportParse(msg) => write!(f, "invalid session export: {msg}"),
            Self::ExportSerialize(msg) => write!(f, "could not serialize truncated history: {msg}"),
        }
    }
}

impl Error for RewindError {}

impl SessionRewindState {
    pub fn turn_count(&self) -> usize {
        self.turn_count
    }

    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    pub fn on_turn_complete(&mut self) {
        self.turn_count += 1;
    }

    pub fn reset(&mut self) {
        self.turn_count = 0;
        self.checkpoints.clear();
        self.next_auto_id = 1;
    }

    pub fn record_checkpoint(&mut self, name: Option<&str>, messages_json: String) -> String {
        let name = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                let n = format!("checkpoint-{}", self.next_auto_id);
                self.next_auto_id += 1;
                n
            });

        if let Some(existing) = self.checkpoints.iter_mut().find(|c| c.name == name) {
            existing.turn_index = self.turn_count;
            existing.messages_json = messages_json;
            return format!("updated checkpoint `{name}` at turn {}", self.turn_count);
        }

        self.checkpoints.push(Checkpoint {
            name: name.clone(),
            turn_index: self.turn_count,
            messages_json,
        });
        format!("checkpoint `{name}` saved at turn {}", self.turn_count)
    }

    pub fn list_checkpoints(&self) -> String {
        if self.checkpoints.is_empty() {
            return format!(
                "no checkpoints yet (turn {}) — use /checkpoint [name] to save the current point",
                self.turn_count
            );
        }

        let mut out = format!("checkpoints (current turn: {}):\n", self.turn_count);
        for cp in &self.checkpoints {
            out.push_str(&format!("  {}  turn {}\n", cp.name, cp.turn_index));
        }
        out.push_str("rewind with /rewind <name> or /rewind <N>");
        out
    }

    pub fn resolve_rewind(&self, arg: &str) -> Result<RewindResolution, RewindError> {
        let arg = arg.trim();
        if arg.is_empty() {
            return Err(RewindError::ListRequested);
        }

        if let Ok(n) = arg.parse::<usize>() {
            if n == 0 {
                return Err(RewindError::InvalidTurns);
            }
            if n > self.turn_count {
                return Err(RewindError::TooManyTurns {
                    requested: n,
                    available: self.turn_count,
                });
            }
            let target_turn = self.turn_count - n;
            if let Some(cp) = self.checkpoints.iter().find(|c| c.turn_index == target_turn) {
                return Ok(RewindResolution::Snapshot {
                    messages_json: cp.messages_json.clone(),
                    target_turn,
                });
            }
            return Ok(RewindResolution::TruncateToTurns { target_turn });
        }

        let cp = self
            .checkpoints
            .iter()
            .find(|c| c.name == arg)
            .ok_or_else(|| RewindError::UnknownCheckpoint(arg.to_string()))?;
        Ok(RewindResolution::Snapshot {
            messages_json: cp.messages_json.clone(),
            target_turn: cp.turn_index,
        })
    }

    pub fn after_rewind(&mut self, target_turn: usize) {
        self.turn_count = target_turn;
        self.checkpoints.retain(|c| c.turn_index <= target_turn);
    }
}

/// Truncate a `session/export` payload to the first `turns_to_keep` user turns.
pub fn truncate_export_to_turns(export_json: &str, turns_to_keep: usize) -> Result<String, RewindError> {
    let parsed = parse_export_json(export_json).map_err(|e| RewindError::ExportParse(e.to_string()))?;
    match parsed {
        ParsedExport::V1(msgs) => {
            let cut = cut_index_for_turns(&msgs.iter().map(|m| m.role.as_str()).collect::<Vec<_>>(), turns_to_keep);
            let truncated = &msgs[..cut];
            serde_json::to_string(truncated).map_err(|e| RewindError::ExportSerialize(e.to_string()))
        }
        ParsedExport::V2(exp) => {
            let roles: Vec<&str> = exp.messages.iter().map(|m| m.role.as_str()).collect();
            let cut = cut_index_for_turns(&roles, turns_to_keep);
            let truncated = trogon_runner_tools::portable_session::PortableExportV2 {
                version: exp.version,
                messages: exp.messages[..cut].to_vec(),
            };
            serde_json::to_string(&truncated).map_err(|e| RewindError::ExportSerialize(e.to_string()))
        }
    }
}

fn cut_index_for_turns(roles: &[&str], turns_to_keep: usize) -> usize {
    let user_indices: Vec<usize> = roles
        .iter()
        .enumerate()
        .filter(|(_, role)| **role == "user")
        .map(|(i, _)| i)
        .collect();
    if turns_to_keep >= user_indices.len() {
        roles.len()
    } else if turns_to_keep == 0 {
        0
    } else {
        user_indices[turns_to_keep]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1_EXPORT: &str = r#"[
        {"role":"user","text":"one"},
        {"role":"assistant","text":"a1"},
        {"role":"user","text":"two"},
        {"role":"assistant","text":"a2"},
        {"role":"user","text":"three"},
        {"role":"assistant","text":"a3"}
    ]"#;

    #[test]
    fn record_checkpoint_assigns_auto_name_and_turn() {
        let mut state = SessionRewindState::default();
        state.on_turn_complete();
        state.on_turn_complete();
        let msg = state.record_checkpoint(None, "[]".to_string());
        assert!(msg.contains("checkpoint-1"));
        assert_eq!(state.checkpoints().len(), 1);
        assert_eq!(state.checkpoints()[0].turn_index, 2);
    }

    #[test]
    fn record_checkpoint_updates_existing_name() {
        let mut state = SessionRewindState::default();
        state.on_turn_complete();
        state.record_checkpoint(Some("before-refactor"), "[]".to_string());
        state.on_turn_complete();
        let msg = state.record_checkpoint(Some("before-refactor"), r#"[{"role":"user","text":"x"}]"#.to_string());
        assert!(msg.contains("updated"));
        assert_eq!(state.checkpoints().len(), 1);
        assert_eq!(state.checkpoints()[0].turn_index, 2);
    }

    #[test]
    fn rewind_by_name_returns_snapshot() {
        let mut state = SessionRewindState::default();
        state.on_turn_complete();
        state.record_checkpoint(Some("save"), r#"[{"role":"user","text":"hi"}]"#.to_string());
        state.on_turn_complete();

        let res = state.resolve_rewind("save").unwrap();
        assert_eq!(
            res,
            RewindResolution::Snapshot {
                messages_json: r#"[{"role":"user","text":"hi"}]"#.to_string(),
                target_turn: 1,
            }
        );
    }

    #[test]
    fn rewind_by_turns_truncates_when_no_checkpoint() {
        let mut state = SessionRewindState::default();
        for _ in 0..3 {
            state.on_turn_complete();
        }

        let res = state.resolve_rewind("2").unwrap();
        assert_eq!(res, RewindResolution::TruncateToTurns { target_turn: 1 });
    }

    #[test]
    fn rewind_by_turns_uses_checkpoint_when_turn_matches() {
        let mut state = SessionRewindState::default();
        state.on_turn_complete();
        state.record_checkpoint(Some("t1"), r#"[{"role":"user","text":"first"}]"#.to_string());
        state.on_turn_complete();
        state.on_turn_complete();

        let res = state.resolve_rewind("2").unwrap();
        assert_eq!(
            res,
            RewindResolution::Snapshot {
                messages_json: r#"[{"role":"user","text":"first"}]"#.to_string(),
                target_turn: 1,
            }
        );
    }

    #[test]
    fn after_rewind_truncates_history_and_prunes_checkpoints() {
        let mut state = SessionRewindState::default();
        for _ in 0..3 {
            state.on_turn_complete();
            state.record_checkpoint(None, "[]".to_string());
        }
        state.after_rewind(1);
        assert_eq!(state.turn_count(), 1);
        assert_eq!(state.checkpoints().len(), 1);
        assert_eq!(state.checkpoints()[0].turn_index, 1);
    }

    #[test]
    fn truncate_export_to_turns_keeps_prefix() {
        let truncated = truncate_export_to_turns(V1_EXPORT, 2).unwrap();
        let parsed = parse_export_json(&truncated).unwrap();
        match parsed {
            ParsedExport::V1(msgs) => {
                assert_eq!(msgs.len(), 4);
                assert_eq!(msgs[0].text, "one");
                assert_eq!(msgs[3].text, "a2");
            }
            ParsedExport::V2(_) => panic!("expected v1"),
        }
    }

    #[test]
    fn truncate_export_to_zero_turns_is_empty() {
        let truncated = truncate_export_to_turns(V1_EXPORT, 0).unwrap();
        assert_eq!(truncated, "[]");
    }

    #[test]
    fn list_checkpoints_empty_and_populated() {
        let state = SessionRewindState::default();
        assert!(state.list_checkpoints().contains("no checkpoints"));

        let mut state = SessionRewindState::default();
        state.on_turn_complete();
        state.record_checkpoint(Some("alpha"), "[]".to_string());
        let listed = state.list_checkpoints();
        assert!(listed.contains("alpha"));
        assert!(listed.contains("turn 1"));
    }

    #[test]
    fn resolve_rewind_empty_arg_lists() {
        let state = SessionRewindState::default();
        assert!(matches!(
            state.resolve_rewind("").unwrap_err(),
            RewindError::ListRequested
        ));
    }
}
