//! Integration tests for trogon-cli repl functions that operate on the real
//! filesystem and a concrete Session implementation.
//!
//! These tests verify `do_init` and `expand_mentions` using actual temp dirs
//! and a minimal in-process Session, which is distinct from the unit tests in
//! `repl.rs` that use the `#[cfg(test)]`-only MockFs and MockSession.
//!
//! Run with:
//!   cargo test -p trogon-cli --test repl_integration

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use trogon_cli::fs::Fs;
use trogon_cli::RealFs;
use trogon_cli::repl::{InitResult, do_init, expand_mentions, handle_slash_command};
use trogon_cli::session::{Session, StreamEvent};

// ── TestFs: RealFs that redirects the trogon config path to a tempdir ────────

/// Implements `Fs` by delegating all I/O to `RealFs`, except reads/writes to
/// the trogon config file (`*.config/trogon/config.json`) which are redirected
/// to `config_file` so tests don't touch the user's real config.
struct TestFs {
    config_file: PathBuf,
}

impl TestFs {
    fn new(config_file: PathBuf) -> Self {
        Self { config_file }
    }

    fn redirect<'a>(&'a self, path: &'a Path) -> &'a Path {
        if path
            .to_str()
            .map(|s| s.ends_with(".config/trogon/config.json"))
            .unwrap_or(false)
        {
            &self.config_file
        } else {
            path
        }
    }
}

impl Fs for TestFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        RealFs.read_to_string(self.redirect(path))
    }

    fn write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        RealFs.write(self.redirect(path), data)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        // Only redirect if the path itself is the config's parent directory.
        // For all other paths (e.g. TROGON.md parent) delegate normally.
        if path
            .to_str()
            .map(|s| {
                s.ends_with(".config/trogon") || s.ends_with(".config/trogon/")
            })
            .unwrap_or(false)
        {
            if let Some(parent) = self.config_file.parent() {
                RealFs.create_dir_all(parent)
            } else {
                Ok(())
            }
        } else {
            RealFs.create_dir_all(path)
        }
    }
}

// ── Minimal in-process Session ────────────────────────────────────────────────

struct QueuedSession {
    id: String,
    turns: Arc<Mutex<Vec<Vec<StreamEvent>>>>,
    recorded_prompts: Arc<Mutex<Vec<String>>>,
}

impl QueuedSession {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            turns: Arc::new(Mutex::new(vec![])),
            recorded_prompts: Arc::new(Mutex::new(vec![])),
        }
    }

    fn queue_turn(&self, events: Vec<StreamEvent>) {
        self.turns.lock().unwrap().push(events);
    }

    fn last_prompt(&self) -> Option<String> {
        self.recorded_prompts.lock().unwrap().last().cloned()
    }
}

// Implement Session using explicit fn → impl Future (not async fn) to satisfy
// the `+ '_` lifetime bound required by the trait's RPITIT signatures.
impl Session for QueuedSession {
    fn session_id(&self) -> &str {
        &self.id
    }

    fn prompt(
        &self,
        text: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<mpsc::Receiver<StreamEvent>>> + Send + '_
    {
        let turns = self.turns.clone();
        let recorded = self.recorded_prompts.clone();
        let text = text.to_string();
        async move {
            recorded.lock().unwrap().push(text);
            let turn = {
                let mut guard = turns.lock().unwrap();
                if guard.is_empty() {
                    vec![StreamEvent::Done("end_turn".into())]
                } else {
                    guard.remove(0)
                }
            };
            let (tx, rx) = mpsc::channel(turn.len().max(1));
            for ev in turn {
                let _ = tx.try_send(ev);
            }
            Ok(rx)
        }
    }

    fn cancel(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
        async move {}
    }

    fn set_model(
        &self,
        _model_id: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_ {
        async move { Ok(()) }
    }

    fn compact(&self) -> impl std::future::Future<Output = anyhow::Result<()>> + Send + '_ {
        async move { Ok(()) }
    }

    fn close(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
        async move {}
    }
}

// ── do_init integration tests ─────────────────────────────────────────────────

/// `do_init` writes TROGON.md to the cwd (or git root) with the AI-generated
/// content, using a real temp directory and a concrete Session.
#[tokio::test]
async fn do_init_integration_writes_trogon_md() {
    let dir = tempfile::tempdir().unwrap();
    let session = QueuedSession::new("init-sess-1");
    session.queue_turn(vec![
        StreamEvent::Text("# MyProject\n## Overview\nA test project.\n".into()),
        StreamEvent::Done("end_turn".into()),
    ]);

    let result = do_init(&session, dir.path(), &RealFs).await.unwrap();

    let dest = match result {
        InitResult::Written(p) => p,
        InitResult::AlreadyExists(_) => panic!("expected Written, got AlreadyExists"),
    };
    let written = std::fs::read_to_string(&dest).unwrap();
    assert!(written.contains("MyProject"), "TROGON.md must contain project name; got: {written}");
    assert!(written.contains("Overview"), "TROGON.md must contain Overview section; got: {written}");
}

/// `do_init` returns `AlreadyExists` when TROGON.md already exists in the cwd.
#[tokio::test]
async fn do_init_integration_returns_already_exists_when_file_present() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("TROGON.md"), "existing content").unwrap();

    let session = QueuedSession::new("init-sess-2");
    let result = do_init(&session, dir.path(), &RealFs).await.unwrap();

    assert!(
        matches!(result, InitResult::AlreadyExists(_)),
        "expected AlreadyExists when TROGON.md already present"
    );
    assert_eq!(
        session.last_prompt(),
        None,
        "prompt must NOT be called when TROGON.md already exists"
    );
}

/// `do_init` strips a markdown code fence from the AI response before writing.
#[tokio::test]
async fn do_init_integration_strips_code_fence() {
    let dir = tempfile::tempdir().unwrap();
    let session = QueuedSession::new("init-sess-3");
    session.queue_turn(vec![
        StreamEvent::Text("```markdown\n# FencedProject\n## Overview\nContent.\n```".into()),
        StreamEvent::Done("end_turn".into()),
    ]);

    let result = do_init(&session, dir.path(), &RealFs).await.unwrap();
    let dest = match result {
        InitResult::Written(p) => p,
        InitResult::AlreadyExists(_) => panic!("expected Written"),
    };

    let written = std::fs::read_to_string(&dest).unwrap();
    assert!(!written.contains("```"), "code fence must be stripped; got: {written}");
    assert!(written.contains("# FencedProject"), "content must be preserved; got: {written}");
}

/// `do_init` includes the detected language (Rust, from Cargo.toml) in the
/// prompt sent to the session. Language detection reads real filesystem files.
#[tokio::test]
async fn do_init_integration_prompt_includes_detected_language() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();

    let session = QueuedSession::new("init-sess-4");
    session.queue_turn(vec![
        StreamEvent::Text("# Demo\n".into()),
        StreamEvent::Done("end_turn".into()),
    ]);

    do_init(&session, dir.path(), &RealFs).await.unwrap();

    let prompt = session.last_prompt().expect("do_init must call session.prompt");
    assert!(
        prompt.contains("Rust"),
        "prompt must mention Rust (detected from Cargo.toml); got: {prompt}"
    );
}

// ── expand_mentions integration tests ────────────────────────────────────────

/// `expand_mentions` injects the content of a real file as a fenced code block
/// when the file referenced by `@filename` exists.
#[test]
fn expand_mentions_integration_real_file_injected_as_code_block() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("context.rs");
    std::fs::write(&file_path, "fn integration_sentinel() {}").unwrap();

    let input = format!("look at @context.rs for context");
    let result = expand_mentions(&input, dir.path(), &RealFs);

    assert!(
        result.contains("integration_sentinel"),
        "expand_mentions must inject real file content; got: {result}"
    );
    assert!(result.contains("```"), "must wrap content in code fence; got: {result}");
    assert!(
        result.contains("`context.rs`"),
        "must include filename in header; got: {result}"
    );
}

/// `expand_mentions` leaves the `@token` unchanged (and prints a warning) when
/// the referenced file does not exist.
#[test]
fn expand_mentions_integration_missing_file_leaves_token_unchanged() {
    let dir = tempfile::tempdir().unwrap();

    let result = expand_mentions("see @nonexistent_file.txt here", dir.path(), &RealFs);

    assert!(
        result.contains("@nonexistent_file.txt"),
        "token must be preserved for missing file; got: {result}"
    );
    assert!(
        !result.contains("```"),
        "no code fence must appear for missing file; got: {result}"
    );
}

/// `expand_mentions` expands multiple @-mentions in a single input string,
/// each referencing a different real file.
#[test]
fn expand_mentions_integration_multiple_real_files_expanded() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("alpha.rs"), "fn alpha() {}").unwrap();
    std::fs::write(dir.path().join("beta.rs"), "fn beta() {}").unwrap();

    let result = expand_mentions("use @alpha.rs and @beta.rs", dir.path(), &RealFs);

    assert!(result.contains("fn alpha()"), "alpha.rs content must be injected; got: {result}");
    assert!(result.contains("fn beta()"), "beta.rs content must be injected; got: {result}");
}

/// Text with no `@` is returned unchanged — no files are read.
#[test]
fn expand_mentions_integration_no_at_sign_passthrough() {
    let dir = tempfile::tempdir().unwrap();
    let input = "just a plain message with no mentions";
    let result = expand_mentions(input, dir.path(), &RealFs);
    assert_eq!(result, input);
}

// ── /config integration tests ─────────────────────────────────────────────────

/// `/config set <key> <value>` writes the key-value to a real JSON file on disk;
/// a subsequent `/config get <key>` reads it back from the same file.
#[test]
fn config_set_persists_to_real_file_and_get_reads_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.json");
    let fs = TestFs::new(config_file.clone());

    let set_out = handle_slash_command("/config", "set theme dark", 0, 0, dir.path(), &fs);
    assert_eq!(set_out, "theme = dark", "set must confirm the assignment; got: {set_out}");

    let written = std::fs::read_to_string(&config_file).expect("config file must exist after set");
    assert!(written.contains("theme"), "config file must contain the key; got: {written}");
    assert!(written.contains("dark"), "config file must contain the value; got: {written}");

    let get_out = handle_slash_command("/config", "get theme", 0, 0, dir.path(), &fs);
    assert_eq!(get_out, "theme = dark", "get must return the persisted value; got: {get_out}");
}

/// `/config set` followed by a second `/config set` on the same key overwrites
/// the value; `/config get` returns the new value.
#[test]
fn config_set_overwrites_existing_value_on_real_disk() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.json");
    let fs = TestFs::new(config_file);

    handle_slash_command("/config", "set model grok-3", 0, 0, dir.path(), &fs);
    handle_slash_command("/config", "set model claude-opus-4-7", 0, 0, dir.path(), &fs);

    let get_out = handle_slash_command("/config", "get model", 0, 0, dir.path(), &fs);
    assert_eq!(
        get_out, "model = claude-opus-4-7",
        "second set must overwrite first; got: {get_out}"
    );
}

/// `/config get <key>` on a key that was never set returns "is not set".
#[test]
fn config_get_missing_key_returns_not_set_from_real_file() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.json");
    let fs = TestFs::new(config_file);

    let out = handle_slash_command("/config", "get nonexistent", 0, 0, dir.path(), &fs);
    assert!(out.contains("not set"), "missing key must say 'not set'; got: {out}");
}

/// `/config` with no args shows the config file path and current JSON contents.
#[test]
fn config_no_args_shows_path_and_contents_from_real_file() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.json");
    let fs = TestFs::new(config_file.clone());

    handle_slash_command("/config", "set env production", 0, 0, dir.path(), &fs);

    let out = handle_slash_command("/config", "", 0, 0, dir.path(), &fs);
    assert!(out.contains("config:"), "output must begin with 'config:'; got: {out}");
    assert!(out.contains("production"), "output must contain the stored value; got: {out}");
}

// ── /help integration test ───────────────────────────────────────────────────

/// `/help` must list all user-facing slash commands. Verified via the real
/// `handle_slash_command` path rather than the `#[cfg(test)]`-only MockFs unit test.
#[test]
fn slash_help_lists_all_required_commands_integration() {
    let dir = tempfile::tempdir().unwrap();
    let out = handle_slash_command("/help", "", 0, 0, dir.path(), &RealFs);

    for cmd in &["/help", "/cost", "/clear", "/compact", "/config", "/model", "/init"] {
        assert!(
            out.contains(cmd),
            "/help output must include '{cmd}'; got:\n{out}"
        );
    }
}

// ── /cost integration tests ───────────────────────────────────────────────────

/// `/cost` with `context_size == 0` returns the "no usage data" message —
/// no real config read is needed.
#[test]
fn cost_no_usage_data_returns_sentinel_message() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.json");
    let fs = TestFs::new(config_file);

    let out = handle_slash_command("/cost", "", 0, 0, dir.path(), &fs);
    assert!(
        out.contains("no usage data"),
        "cost with no data must say 'no usage data'; got: {out}"
    );
}

/// `/cost` with real token counts reads the model from the on-disk config to
/// compute the rate, formats `used/total (pct%)  |  ~$cost`, and the dollar
/// amount matches the blended rate for the configured model.
#[test]
fn cost_reads_model_from_real_config_and_formats_output() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.json");
    let fs = TestFs::new(config_file);

    // Set model to opus (rate = $28.5/Mtoken).
    handle_slash_command("/config", "set model claude-opus-4-7", 0, 0, dir.path(), &fs);

    // 500_000 tokens at $28.5/Mtoken = $14.25
    let out = handle_slash_command("/cost", "", 500_000, 1_000_000, dir.path(), &fs);

    assert!(out.contains("500"), "used tokens must appear; got: {out}");
    assert!(out.contains("1,000"), "context size must appear; got: {out}");
    assert!(out.contains("50%"), "percentage must be 50%; got: {out}");
    assert!(out.contains("14.25"), "cost must be $14.25 for opus at 500k tokens; got: {out}");
}

/// `/cost` with no config file (first run) defaults to the sonnet rate.
#[test]
fn cost_defaults_to_sonnet_rate_when_no_config_exists() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.json"); // does not exist
    let fs = TestFs::new(config_file);

    // 1_000_000 tokens at sonnet rate $6.0/Mtoken = $6.00
    let out = handle_slash_command("/cost", "", 1_000_000, 2_000_000, dir.path(), &fs);

    assert!(out.contains("6.00"), "default sonnet rate must produce $6.00 for 1M tokens; got: {out}");
}

// ── REPL history persistence ──────────────────────────────────────────────────

/// REPL history persistence: `save_history` writes entries to disk and
/// `load_history` reads them back in a fresh editor session.
///
/// Mirrors the exact mechanism in `run_repl` (repl.rs lines 146–153, 234, 310):
/// `load_history` on startup → `add_history_entry` per prompt → `save_history`
/// on exit. Tests the file I/O round-trip end-to-end without requiring a TTY.
#[test]
fn repl_history_persists_to_disk_and_reloads_across_sessions() {
    let dir = tempfile::TempDir::new().unwrap();
    let history_file = dir.path().join("trogon_history");

    // ── 1. First session: add entries and save to disk ────────────────────────
    {
        let mut rl = rustyline::DefaultEditor::new()
            .expect("rustyline DefaultEditor::new must succeed");
        rl.add_history_entry("explain the codebase").unwrap();
        rl.add_history_entry("write tests for the parser").unwrap();
        rl.add_history_entry("run the build").unwrap();
        rl.save_history(&history_file)
            .expect("save_history must succeed");
    }

    // History file must exist and be non-empty after save.
    assert!(
        history_file.exists(),
        "history file must exist on disk after save_history"
    );
    let raw = std::fs::read_to_string(&history_file)
        .expect("history file must be readable");
    assert!(
        raw.contains("explain the codebase"),
        "history file must contain first entry; got: {raw}"
    );
    assert!(
        raw.contains("write tests for the parser"),
        "history file must contain second entry; got: {raw}"
    );
    assert!(
        raw.contains("run the build"),
        "history file must contain third entry; got: {raw}"
    );

    // ── 2. Second session: load from the same file — must not error ──────────
    {
        let mut rl = rustyline::DefaultEditor::new()
            .expect("rustyline DefaultEditor::new must succeed");
        rl.load_history(&history_file)
            .expect("load_history must succeed on existing history file");
    }
}
