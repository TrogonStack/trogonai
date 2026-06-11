//! Optional per-session REPL transcript recorder.
//!
//! When `TROGON_TRANSCRIPT=1` (or `true`/`yes`/`on`), each interactive turn is
//! appended as one JSON line to `~/.local/share/trogon/transcripts/<session_id>.jsonl`.

use crate::app::TurnStop;
use crate::fs::Fs;
use crate::session_store::{expand_tilde, new_session_entry};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const TRANSCRIPTS_DIR: &str = "~/.local/share/trogon/transcripts";

/// True when `TROGON_TRANSCRIPT` is set to a truthy value.
pub fn transcript_enabled() -> bool {
    std::env::var("TROGON_TRANSCRIPT")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// One REPL turn: user input, assistant text, and how the turn ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TranscriptTurn {
    pub timestamp: String,
    pub user: String,
    pub assistant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Append-only per-session transcript writer. Disabled unless
/// [`transcript_enabled`] is true.
pub struct SessionTranscriptRecorder<'a, F: Fs + ?Sized> {
    path: PathBuf,
    fs: &'a F,
}

impl<'a, F: Fs + ?Sized> SessionTranscriptRecorder<'a, F> {
    /// Build a recorder when transcript capture is enabled; otherwise `None`.
    pub fn for_session(fs: &'a F, session_id: &str) -> Option<Self> {
        if !transcript_enabled() {
            return None;
        }
        Some(Self::new(fs, session_id))
    }

    pub(crate) fn new(fs: &'a F, session_id: &str) -> Self {
        let path = transcript_path(session_id);
        Self { path, fs }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one turn. Write failures are logged and ignored.
    pub fn record_turn(
        &self,
        user: &str,
        assistant: &str,
        stop: Option<&TurnStop>,
        interrupted: bool,
    ) {
        let (stop_reason, error) = match stop {
            Some(TurnStop::Done { reason }) => (Some(reason.clone()), None),
            Some(TurnStop::Error(msg)) => (None, Some(msg.clone())),
            None if interrupted => (Some("interrupted".into()), None),
            None => (None, None),
        };
        let turn = TranscriptTurn {
            timestamp: new_session_entry("", "", "").updated_at,
            user: user.to_string(),
            assistant: assistant.to_string(),
            stop_reason,
            error,
        };
        if let Err(e) = self.append_turn(&turn) {
            tracing::warn!(error = %e, path = %self.path.display(), "failed to write REPL transcript");
        }
    }

    fn append_turn(&self, turn: &TranscriptTurn) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            self.fs.create_dir_all(dir)?;
        }
        let mut line = serde_json::to_string(turn).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        line.push('\n');
        let mut contents = self.fs.read_to_string(&self.path).unwrap_or_default();
        contents.push_str(&line);
        self.fs.write(&self.path, contents.as_bytes())
    }
}

pub fn transcript_path(session_id: &str) -> PathBuf {
    expand_tilde(TRANSCRIPTS_DIR).join(format!("{session_id}.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::TurnStop;
    use crate::fs::mock::MockFs;
    use std::io;

    struct FailingFs;

    impl Fs for FailingFs {
        fn read_to_string(&self, _path: &Path) -> io::Result<String> {
            Ok(String::new())
        }

        fn write(&self, _path: &Path, _contents: &[u8]) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
        }

        fn create_dir_all(&self, _path: &Path) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn disabled_is_noop() {
        let fs = MockFs::new();
        unsafe { std::env::remove_var("TROGON_TRANSCRIPT") };
        assert!(!transcript_enabled());
        assert!(SessionTranscriptRecorder::for_session(&fs, "sess-1").is_none());
    }

    #[test]
    fn enabled_recognizes_truthy_values() {
        unsafe { std::env::set_var("TROGON_TRANSCRIPT", "1") };
        assert!(transcript_enabled());
        unsafe { std::env::set_var("TROGON_TRANSCRIPT", "TRUE") };
        assert!(transcript_enabled());
        unsafe { std::env::remove_var("TROGON_TRANSCRIPT") };
    }

    #[test]
    fn writes_turn_line() {
        unsafe { std::env::remove_var("TROGON_TRANSCRIPT") };
        let fs = MockFs::new();
        let recorder = SessionTranscriptRecorder::new(&fs, "sess-abc");
        recorder.record_turn(
            "user q",
            "assistant a",
            Some(&TurnStop::Done {
                reason: "end_turn".into(),
            }),
            false,
        );

        let content = fs.read_bytes(recorder.path()).expect("transcript file");
        let line = String::from_utf8(content).unwrap();
        assert!(line.contains("\"user\":\"user q\""));
        assert!(line.contains("\"assistant\":\"assistant a\""));
        assert!(line.contains("\"stop_reason\":\"end_turn\""));
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn write_error_does_not_panic() {
        let fs = FailingFs;
        let recorder = SessionTranscriptRecorder::new(&fs, "sess-1");
        recorder.record_turn("hi", "hello", None, false);
    }
}
