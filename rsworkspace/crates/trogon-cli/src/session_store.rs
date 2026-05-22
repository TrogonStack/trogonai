//! Local session index for `--continue` and cross-runner resume.
//!
//! Persists the last active session per project at `~/.local/share/trogon/sessions.json`.

use crate::fs::Fs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const SESSIONS_PATH: &str = "~/.local/share/trogon/sessions.json";

/// One indexed session (runner prefix + id + model at time of last activity).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEntry {
    pub prefix: String,
    pub session_id: String,
    pub model: String,
    pub updated_at: String,
}

/// Per-project session index with optional per-runner entries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSessions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<SessionEntry>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub by_prefix: HashMap<String, SessionEntry>,
}

/// On-disk index keyed by canonical project path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionIndex {
    #[serde(flatten)]
    projects: HashMap<String, ProjectSessions>,
}

impl SessionIndex {
    pub fn path() -> PathBuf {
        expand_tilde(SESSIONS_PATH)
    }

    pub fn load<F: Fs>(fs: &F) -> Self {
        let path = Self::path();
        match fs.read_to_string(&path) {
            Ok(raw) if !raw.trim().is_empty() => {
                serde_json::from_str(&raw).unwrap_or_default()
            }
            _ => Self::default(),
        }
    }

    pub fn save<F: Fs>(&self, fs: &F) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            fs.create_dir_all(dir)?;
        }
        let raw = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        fs.write(&path, raw.as_bytes())
    }

    pub fn get_last(&self, project: &Path) -> Option<&SessionEntry> {
        self.projects.get(&project_key(project))?.last.as_ref()
    }

    pub fn get_for_prefix(&self, project: &Path, prefix: &str) -> Option<&SessionEntry> {
        self.projects
            .get(&project_key(project))
            .and_then(|p| p.by_prefix.get(prefix))
    }

    /// Record `entry` as the latest session for `project` and for its runner prefix.
    pub fn record(&mut self, project: &Path, entry: SessionEntry) {
        let key = project_key(project);
        let slot = self.projects.entry(key).or_default();
        slot.by_prefix.insert(entry.prefix.clone(), entry.clone());
        slot.last = Some(entry);
    }
}

pub fn project_key(cwd: &Path) -> String {
    cwd.canonicalize()
        .unwrap_or_else(|_| cwd.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| {
            let secs = d.as_secs();
            let days = secs / 86_400;
            let time = secs % 86_400;
            let (y, mo, day) = civil_from_days(days as i64);
            let h = time / 3600;
            let min = (time % 3600) / 60;
            let s = time % 60;
            format!("{y:04}-{mo:02}-{day:02}T{h:02}:{min:02}:{s:02}Z")
        })
        .unwrap_or_default()
}

/// Civil date from days since Unix epoch (1970-01-01).
fn civil_from_days(z: i64) -> (u32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + (era * 400) as u32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mp < 10 { y + 1 } else { y };
    (y, m, d)
}

pub fn new_session_entry(prefix: impl Into<String>, session_id: impl Into<String>, model: impl Into<String>) -> SessionEntry {
    SessionEntry {
        prefix: prefix.into(),
        session_id: session_id.into(),
        model: model.into(),
        updated_at: now_iso8601(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::mock::MockFs;

    #[test]
    fn record_and_get_last_round_trip() {
        let mut index = SessionIndex::default();
        let project = PathBuf::from("/tmp/proj");
        let entry = new_session_entry("acp.claude", "sess-1", "claude-sonnet-4-6");
        index.record(&project, entry.clone());
        assert_eq!(index.get_last(&project), Some(&entry));
        assert_eq!(
            index.get_for_prefix(&project, "acp.claude"),
            Some(&entry)
        );
    }

    #[test]
    fn save_and_load_persists_index() {
        let fs = MockFs::new();
        let project = PathBuf::from("/workspace/app");
        let entry = new_session_entry("acp.grok", "sess-grok", "grok-3");

        let mut index = SessionIndex::default();
        index.record(&project, entry.clone());
        index.save(&fs).unwrap();

        let loaded = SessionIndex::load(&fs);
        assert_eq!(loaded.get_last(&project), Some(&entry));
    }

    #[test]
    fn load_missing_file_returns_empty_index() {
        let fs = MockFs::new();
        assert!(SessionIndex::load(&fs).get_last(Path::new("/none")).is_none());
    }

    #[test]
    fn by_prefix_tracks_multiple_runners() {
        let mut index = SessionIndex::default();
        let project = PathBuf::from("/repo");
        let claude = new_session_entry("acp.claude", "c1", "sonnet");
        let grok = new_session_entry("acp.grok", "g1", "grok-3");
        index.record(&project, claude.clone());
        index.record(&project, grok.clone());
        assert_eq!(index.get_last(&project), Some(&grok));
        assert_eq!(index.get_for_prefix(&project, "acp.claude"), Some(&claude));
    }
}
