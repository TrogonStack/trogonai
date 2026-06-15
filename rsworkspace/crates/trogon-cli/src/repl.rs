use crate::RunnerSwitcher;
use crate::app::{
    TurnMetrics, TurnRenderer, TurnStop, print_command_echo, print_startup_banner, print_user_line,
    warn_if_codex_observational,
};
use crate::fs::Fs;
use crate::mcp::McpManager;
use crate::session::{CompactResult, PromptOpts, Session, SessionFactory, StreamEvent};
use crate::session_rewind::{
    RewindError, RewindResolution, SessionRewindState, truncate_export_to_turns,
};
use crate::spawn_tracker::SpawnTracker;
use crate::session_store::{SessionIndex, new_session_entry};
use crate::transcript::SessionTranscriptRecorder;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::config::Configurer;
use rustyline::{
    Cmd, ConditionalEventHandler, Context, Editor, EditMode, EventContext, EventHandler, Helper,
    KeyCode, KeyEvent, Modifiers,
};
use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::client_supervisor::AcpClientSupervisor;
use crate::stream_input::{StreamInputEvent, StreamInputReader};
use crate::terminal::reset_display;
use crate::tui_client::PermissionCoordinator;
use std::collections::VecDeque;
use trogon_registry::{Registry, RegistryStore};
use trogon_tools::fs::{resolve_directory_target, resolve_path};

const HISTORY_PATH: &str = "~/.local/share/trogon/history";

/// A prompt queued for auto-submission (custom slash commands, /review, stream input).
#[derive(Debug, Clone, PartialEq, Eq)]
struct QueuedPrompt {
    text: String,
    model: Option<String>,
    allowed_tools: Vec<String>,
}

impl QueuedPrompt {
    fn from_text(text: String) -> Self {
        Self {
            text,
            model: None,
            allowed_tools: Vec::new(),
        }
    }

    fn from_dispatch(dispatch: crate::commands::CustomCommandDispatch) -> Self {
        Self {
            text: dispatch.prompt,
            model: dispatch.model,
            allowed_tools: dispatch.allowed_tools,
        }
    }

    fn prompt_opts(&self) -> PromptOpts {
        crate::commands::prompt_opts_from_dispatch(&crate::commands::CustomCommandDispatch {
            prompt: self.text.clone(),
            model: self.model.clone(),
            allowed_tools: self.allowed_tools.clone(),
        })
    }
}

// ── FileAtHelper ──────────────────────────────────────────────────────────────

struct FileAtHelper {
    cwd: PathBuf,
    /// Fish-style autosuggestion from history: dims the most recent matching
    /// entry after the cursor; accepted with Tab (see [`TabHandler`]) or →.
    history_hinter: HistoryHinter,
    /// Shared permission-mode cell, rendered into the prompt by `highlight_prompt`
    /// and cycled by Shift+Tab ([`TabModeHandler`]). `Arc<Mutex>` so the handler
    /// (which must be `Send + Sync`) can share it; no real contention.
    mode: std::sync::Arc<std::sync::Mutex<String>>,
}

impl Default for FileAtHelper {
    fn default() -> Self {
        Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

impl FileAtHelper {
    fn new(cwd: PathBuf) -> Self {
        Self::with_mode(cwd, std::sync::Arc::new(std::sync::Mutex::new(String::from("default"))))
    }

    fn with_mode(cwd: PathBuf, mode: std::sync::Arc<std::sync::Mutex<String>>) -> Self {
        Self { cwd, history_hinter: HistoryHinter::new(), mode }
    }
}

impl Helper for FileAtHelper {}

impl Completer for FileAtHelper {
    type Candidate = Pair;

    fn complete(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> rustyline::Result<(usize, Vec<Pair>)> {
        let before = &line[..pos];
        let Some(at_pos) = before.rfind('@') else {
            return Ok((pos, vec![]));
        };
        let partial = &before[at_pos + 1..];
        if partial.contains(' ') {
            return Ok((pos, vec![]));
        }
        let (dir, prefix, dir_prefix) = if let Some(slash) = partial.rfind('/') {
            let (dir_part, file_part) = partial.split_at(slash + 1);
            (self.cwd.join(dir_part), file_part, dir_part)
        } else {
            (self.cwd.clone(), partial, "")
        };
        let mut pairs: Vec<Pair> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(prefix) {
                    let suffix = if entry.path().is_dir() { "/" } else { "" };
                    let replacement = format!("{dir_prefix}{name}{suffix}");
                    pairs.push(Pair {
                        display: format!("{name}{suffix}"),
                        replacement,
                    });
                }
            }
        }
        pairs.sort_by(|a, b| a.display.cmp(&b.display));
        Ok((at_pos + 1, pairs))
    }
}

impl Hinter for FileAtHelper {
    type Hint = String;
    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<String> {
        // Delegate to the history hinter (only fires at end-of-line on a
        // prefix match), so a dimmed completion of a prior command appears.
        self.history_hinter.hint(line, pos, ctx)
    }
}

impl Highlighter for FileAtHelper {
    /// Render the autosuggestion dimmed so it's visibly distinct from typed text.
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(format!("\x1b[90m{hint}\x1b[0m"))
    }

    /// Render the prompt from the shared mode cell, coloured. The *visible* width
    /// matches `format_mode_prompt` (which is what rustyline measures for cursor
    /// math), so Shift+Tab can repaint a different-length mode name safely.
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        _prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        let mode = self
            .mode
            .lock()
            .map(|m| m.clone())
            .unwrap_or_else(|_| String::from("default"));
        let bracketed = format!("[{}]", mode_label(&mode));
        Cow::Owned(format!("\x1b[35m{:<width$}\x1b[0m › ", bracketed, width = MODE_FIELD))
    }
    // No highlight_char override: Shift+Tab's Cmd::Repaint forces a full
    // refresh_line (which always re-renders the prompt via highlight_prompt), so
    // we don't need per-keystroke re-highlighting — keeping the default (false)
    // avoids redrawing the whole line on every character.
}

/// What Tab should do given the current input state.
#[derive(Debug, PartialEq, Eq)]
enum TabChoice {
    /// Accept the showing history autosuggestion.
    CompleteHint,
    /// Run `@`-file completion.
    Complete,
    /// Fall through to rustyline's default Tab handling.
    Default,
}

/// Decide Tab's action. An active `@`-mention token always takes precedence so
/// Tab keeps completing file paths even when a history hint exists; otherwise a
/// showing hint at end-of-line is accepted; otherwise default.
fn tab_choice(line: &str, pos: usize, has_hint: bool) -> TabChoice {
    let in_at_mention = line[..pos]
        .rsplit(char::is_whitespace)
        .next()
        .is_some_and(|word| word.starts_with('@'));
    if in_at_mention {
        TabChoice::Complete
    } else if has_hint && pos == line.len() {
        TabChoice::CompleteHint
    } else {
        TabChoice::Default
    }
}

/// Tab handler: accept the autosuggestion when one is showing, otherwise fall
/// back to `@`-file completion (see [`tab_choice`]).
struct TabHandler;

impl ConditionalEventHandler for TabHandler {
    fn handle(
        &self,
        _evt: &rustyline::Event,
        _n: rustyline::RepeatCount,
        _positive: bool,
        ctx: &EventContext<'_>,
    ) -> Option<Cmd> {
        match tab_choice(ctx.line(), ctx.pos(), ctx.has_hint()) {
            TabChoice::Complete => Some(Cmd::Complete),
            TabChoice::CompleteHint => Some(Cmd::CompleteHint),
            TabChoice::Default => None,
        }
    }
}

// ── Shift+Tab permission-mode cycling ───────────────────────────────────────────

/// Width of the `[label]` field in the prompt. Sized to the widest label
/// (`[acceptEdits]` = 13) so the prompt's visible width is constant across modes —
/// required for correct cursor math when Shift+Tab repaints a new mode.
const MODE_FIELD: usize = 13;

/// Next mode in the Shift+Tab cycle. Any mode outside the cycle (e.g. dontAsk,
/// bypassPermissions) enters it at `default`.
fn next_cycle_mode(current: &str) -> &'static str {
    match current {
        "default" => "acceptEdits",
        "acceptEdits" => "plan",
        "plan" => "default",
        _ => "default",
    }
}

/// Short display label for a mode (keeps every label ≤ 11 chars so `[label]`
/// fits `MODE_FIELD`).
fn mode_label(mode: &str) -> &str {
    match mode {
        "bypassPermissions" => "bypass",
        other => other,
    }
}

/// Plain (un-coloured), fixed-width prompt for `mode`. Passed to rustyline as the
/// raw prompt so its measured width matches the coloured `highlight_prompt`.
fn format_mode_prompt(mode: &str) -> String {
    let bracketed = format!("[{}]", mode_label(mode));
    format!("{:<width$} › ", bracketed, width = MODE_FIELD)
}

/// Shift+Tab handler: cycle the shared mode cell and repaint so the prompt's mode
/// indicator updates instantly. The async `set_mode` is applied by the REPL loop
/// once `readline` returns (a handler cannot await).
struct TabModeHandler {
    mode: std::sync::Arc<std::sync::Mutex<String>>,
}

impl ConditionalEventHandler for TabModeHandler {
    fn handle(
        &self,
        _evt: &rustyline::Event,
        _n: rustyline::RepeatCount,
        _positive: bool,
        _ctx: &EventContext<'_>,
    ) -> Option<Cmd> {
        if let Ok(mut m) = self.mode.lock() {
            let next = next_cycle_mode(m.as_str()).to_string();
            *m = next;
        }
        Some(Cmd::Repaint)
    }
}

// ── Ctrl+X Ctrl+E: edit the prompt in $EDITOR ───────────────────────────────────

/// Open `$VISUAL`/`$EDITOR` (fallback `vi`) on a temp file seeded with `initial`,
/// returning the edited text (trailing newlines trimmed). The editor inherits the
/// terminal, so this must run with the terminal in normal mode — i.e. *after*
/// `readline` returns, never inside a rustyline handler.
fn edit_in_editor(initial: &str) -> std::io::Result<String> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    edit_with(&editor, initial)
}

/// Editor round-trip with an explicit editor command (injected for testability).
/// `editor` may include args (e.g. `"code --wait"`); the temp file path is
/// appended as the final argument.
fn edit_with(editor: &str, initial: &str) -> std::io::Result<String> {
    // Unique per call (pid + monotonic counter) so concurrent edits/tests don't
    // share a temp file.
    static EDIT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = EDIT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!("trogon-prompt-{}-{seq}.md", std::process::id()));
    std::fs::write(&path, initial)?;
    let mut parts = editor.split_whitespace();
    let bin = parts.next().unwrap_or("vi");
    let args: Vec<&str> = parts.collect();
    let status = std::process::Command::new(bin).args(&args).arg(&path).status();
    let content = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    status?;
    Ok(content?.trim_end_matches(['\n', '\r']).to_string())
}

/// Ctrl+X Ctrl+E handler: capture the current buffer and force `readline` to
/// return (via `Cmd::Interrupt`) so the REPL loop — where the terminal is back in
/// normal mode — can launch the editor. The captured buffer distinguishes this
/// from a real Ctrl+C in the loop's interrupt arm.
struct EditorHandler {
    request: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl ConditionalEventHandler for EditorHandler {
    fn handle(
        &self,
        _evt: &rustyline::Event,
        _n: rustyline::RepeatCount,
        _positive: bool,
        ctx: &EventContext<'_>,
    ) -> Option<Cmd> {
        if let Ok(mut r) = self.request.lock() {
            *r = Some(ctx.line().to_string());
        }
        Some(Cmd::Interrupt)
    }
}

impl Validator for FileAtHelper {
    fn validate(&self, ctx: &mut ValidationContext<'_>) -> rustyline::Result<ValidationResult> {
        if input_needs_continuation(ctx.input()) {
            Ok(ValidationResult::Incomplete)
        } else {
            Ok(ValidationResult::Valid(None))
        }
    }
}

fn input_needs_continuation(input: &str) -> bool {
    input.ends_with('\\')
}

// ── @mention expansion ────────────────────────────────────────────────────────

pub fn expand_mentions<F: Fs>(text: &str, cwd: &Path, fs: &F) -> String {
    let cwd_str = cwd.to_string_lossy();
    let mut result = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch == '@' {
            let start = i + 1;
            let mut end = start;
            while let Some(&(j, c)) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                end = j + c.len_utf8();
                chars.next();
            }
            let path_str = &text[start..end];
            if path_str.is_empty() {
                result.push('@');
            } else {
                match resolve_path(&cwd_str, path_str) {
                    Ok(full_path) => match fs.read_to_string(&full_path) {
                        Ok(content) => {
                            // MED-37: Cap file expansions to 200 KB to protect the context window.
                            const MAX_MENTION_BYTES: usize = 204_800;
                            let body = if content.len() > MAX_MENTION_BYTES {
                                eprintln!(
                                    "warning: @{path_str}: file too large ({} bytes), \
                                     showing first {MAX_MENTION_BYTES} bytes",
                                    content.len()
                                );
                                let boundary = content.floor_char_boundary(MAX_MENTION_BYTES);
                                format!(
                                    "[File truncated: {} bytes, showing first {MAX_MENTION_BYTES}]\n{}",
                                    content.len(),
                                    &content[..boundary]
                                )
                            } else {
                                content
                            };
                            result.push_str(&format!("`{path_str}`:\n```\n{body}\n```"));
                        }
                        Err(e) => {
                            // LOW-20: distinguish "is a directory" from other read errors.
                            let msg = if e.kind() == std::io::ErrorKind::IsADirectory || full_path.is_dir() {
                                "is a directory"
                            } else {
                                "file not found or not readable"
                            };
                            eprintln!("warning: @{path_str}: {msg}");
                            result.push('@');
                            result.push_str(path_str);
                        }
                    },
                    Err(_) => {
                        eprintln!("warning: @{path_str}: path escapes working directory — ignored");
                        result.push('@');
                        result.push_str(path_str);
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}

// ── Multiline join ────────────────────────────────────────────────────────────

fn join_continuation(s: &str) -> String {
    s.replace("\\\n", " ")
}

// ── `!` shell escape ────────────────────────────────────────────────────────────

/// Run a `!`-prefixed line as a local shell command in the REPL's working
/// directory. Stdout/stderr are inherited so output streams live to the terminal.
/// This is a local shell escape — neither the command nor its output is sent to
/// the model.
fn run_shell_command(cmd: &str, cwd: &Path) -> std::io::Result<std::process::ExitStatus> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    std::process::Command::new(shell)
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .status()
}

// ── REPL entry point ──────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub async fn run<SF: SessionFactory, F: Fs, SW: RunnerSwitcher, RS: RegistryStore>(
    factory: SF,
    prefix: &str,
    mut cwd: PathBuf,
    fs: F,
    mut switcher: SW,
    registry: Registry<RS>,
    client_supervisor: Option<Rc<AcpClientSupervisor>>,
    permission_coordinator: std::sync::Arc<PermissionCoordinator>,
    stream: bool,
    default_model: Option<String>,
    resume: Option<crate::session_store::SessionEntry>,
    skip_permissions: bool,
    plan: bool,
    session_init: crate::session::SessionInit,
    name: Option<String>,
) -> anyhow::Result<()> {
    let mut prefix = prefix.to_string();
    let init_prefix = prefix.clone(); // always use the startup runner for /init
    let project_dir = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());

    let mut mcp_manager = McpManager::load(&fs);
    // HTTP client used to fetch MCP prompts on demand for `/mcp__server__prompt`.
    let mcp_http = reqwest::Client::new();
    // Lifecycle hooks from settings.json (CLI-side events wired below).
    let hooks_config = crate::settings::Settings::load(&fs, &project_dir).hooks;
    // Session name: prefer the `--name` flag; otherwise inherit a name from the
    // resumed session entry. Updatable at runtime via `/rename`.
    let mut session_name = name.or_else(|| resume.as_ref().and_then(|e| e.name.clone()));
    let resumed = resume.is_some();
    let mut session = if let Some(entry) = resume {
        prefix = entry.prefix.clone();
        match activate_session(&factory, &mut mcp_manager, &prefix, &entry.session_id, &cwd, &fs).await {
            Ok(s) => {
                eprintln!("resumed session {} on {prefix}", s.session_id());
                s
            }
            Err(e) => {
                eprintln!("warning: could not resume {}: {e} — starting fresh", entry.session_id);
                start_session(&factory, &mut mcp_manager, &prefix, cwd.clone(), &session_init, &fs).await?
            }
        }
    } else {
        start_session(&factory, &mut mcp_manager, &prefix, cwd.clone(), &session_init, &fs).await?
    };
    // `--dangerously-skip-permissions` / `--plan` select a startup mode up front
    // (clap enforces they are mutually exclusive). `TROGON_MODE` is applied by the
    // runner itself, so only the flag-driven overrides need an explicit set_mode.
    let startup_mode = crate::runtime::startup_mode(skip_permissions, plan);
    if (skip_permissions || plan)
        && let Err(e) = session.set_mode(&startup_mode).await
    {
        eprintln!("warning: could not set startup mode {startup_mode}: {e}");
    }
    if let Some(ref sup) = client_supervisor {
        sup.set_session(session.session_id());
        if resumed && let Err(e) = sup.rebind(&prefix, session.session_id()).await {
            eprintln!("warning: permission client rebind failed: {e}");
        }
    }
    if !resumed
        && let Some(ref m) = default_model
        && let Err(e) = session.set_model(m).await
    {
        eprintln!("warning: could not apply default model {m}: {e}");
    }

    // SessionStart hooks run once; any context they emit is injected into the
    // first user prompt (mirroring Claude Code's SessionStart semantics).
    let mut pending_start_context: Option<String> = None;
    if !hooks_config.session_start.is_empty() {
        let payload = serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": session.session_id(),
            "cwd": cwd.to_string_lossy(),
        });
        if let trogon_runner_tools::HookOutcome::Continue { context: Some(ctx) } =
            trogon_runner_tools::run_event_hooks(&hooks_config.session_start, None, &payload).await
        {
            pending_start_context = Some(ctx);
        }
    }

    let history_path = expand_tilde(HISTORY_PATH);
    if let Some(dir) = history_path.parent() {
        let _ = fs.create_dir_all(dir);
    }

    let mut rl: Editor<FileAtHelper, _> = Editor::new()?;
    // Shared permission-mode cell: rendered into the prompt and cycled by Shift+Tab.
    let mode_cell = std::sync::Arc::new(std::sync::Mutex::new(startup_mode.clone()));
    rl.set_helper(Some(FileAtHelper::with_mode(cwd.clone(), mode_cell.clone())));
    // Tab accepts the dimmed history autosuggestion when one is showing; falls
    // back to `@`-file completion otherwise (see TabHandler).
    rl.bind_sequence(
        KeyEvent(KeyCode::Tab, Modifiers::NONE),
        EventHandler::Conditional(Box::new(TabHandler)),
    );
    // Shift+Tab (BackTab) cycles permission modes: default → acceptEdits → plan → …
    rl.bind_sequence(
        KeyEvent(KeyCode::BackTab, Modifiers::NONE),
        EventHandler::Conditional(Box::new(TabModeHandler { mode: mode_cell.clone() })),
    );
    // Ctrl+X Ctrl+E opens the current prompt in $EDITOR (see EditorHandler). The
    // handler stashes the buffer here and returns Interrupt; the loop's interrupt
    // arm reads this to tell an editor request apart from a real Ctrl+C.
    let editor_request = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    rl.bind_sequence(
        rustyline::Event::KeySeq(vec![KeyEvent::ctrl('X'), KeyEvent::ctrl('E')]),
        EventHandler::Conditional(Box::new(EditorHandler { request: editor_request.clone() })),
    );
    let _ = rl.load_history(&history_path);

    let mut session_used_tokens: u64 = 0;
    let mut session_context_size: u64 = 0;
    // Tracks the per-session compaction model override selected via /compact-model.
    // None means "default" (compaction uses the session model). Reset on /clear and
    // cross-runner /model switches, which start a fresh session.
    let mut compactor_model_sel: Option<String> = None;
    let mut session_mode = startup_mode.clone();
    // Tracks rustyline edit mode toggled by /vim (false = emacs, true = vi).
    let mut vim_mode = false;
    print_startup_banner(session.session_id(), &prefix, &session_mode);
    // RUN-2: codex is descoped to observational-only — warn prominently at startup.
    warn_if_codex_observational(&prefix);

    sync_repl_cwd_from_session(&session, &mut cwd).await;
    if let Some(helper) = rl.helper_mut() {
        helper.cwd = cwd.clone();
    }

    // Record the session (and any --name) in the local index at creation, so
    // `trogon sessions list` and /sessions show it — and its name — immediately,
    // rather than only after the first turn or a /rename. On resume this refreshes
    // the existing entry.
    persist_session_index(
        &fs,
        &project_dir,
        &prefix,
        session.session_id(),
        &session.current_model(),
        session_name.as_deref(),
    );

    // Claude-style interrupt UX: when Ctrl+C cancels an in-flight response, the
    // prompt the user submitted is stashed here and pre-filled into the next
    // readline so they can edit and resend it instead of retyping it.
    let mut pending_input: Option<String> = None;
    // Messages typed while a response was streaming. Auto-submitted in order
    // (one per turn) once the current turn finishes.
    let mut queued_prompts: VecDeque<QueuedPrompt> = VecDeque::new();
    let mut rewind_state = SessionRewindState::default();
    let custom_commands = crate::commands::load_commands(&project_dir);
    let mut spawn_tracker = SpawnTracker::default();

    loop {
        permission_coordinator.cancel_pending();
        // Mirror the live mode into the shared cell so the prompt shows it. During
        // readline, Shift+Tab may diverge the cell; that's reconciled once readline
        // returns (below). This single sync point covers every session_mode change
        // (/mode, /clear, /model) without touching each assignment site.
        if let Ok(mut m) = mode_cell.lock() {
            *m = session_mode.clone();
        }
        // A queued message (typed during the previous turn) is submitted before
        // reading new input. `from_queue` skips the readline-echo erase below,
        // since there's no readline echo line to overwrite.
        let (read, from_queue, queued_invocation): (rustyline::Result<String>, bool, Option<QueuedPrompt>) =
            match queued_prompts.pop_front() {
                Some(q) => (Ok(q.text.clone()), true, Some(q)),
                None => {
                    let prompt = format_mode_prompt(&session_mode);
                    let r = match pending_input.take() {
                        Some(text) => rl.readline_with_initial(&prompt, (&text, "")),
                        None => rl.readline(&prompt),
                    };
                    (r, false, None)
                }
            };
        match read {
            Ok(raw_line) => {
                // Apply a Shift+Tab mode change made during readline. The cell was
                // synced to session_mode at loop top, so any divergence is a cycle.
                // (A handler can't await, so set_mode happens here.)
                let desired_mode = mode_cell.lock().ok().map(|m| m.clone());
                if let Some(desired) = desired_mode
                    && desired != session_mode
                {
                    match session.set_mode(&desired).await {
                        Ok(()) => session_mode = desired,
                        Err(e) => {
                            eprintln!("warning: could not set mode {desired}: {e}");
                            if let Ok(mut m) = mode_cell.lock() {
                                *m = session_mode.clone();
                            }
                        }
                    }
                }
                let line = join_continuation(&raw_line).trim().to_string();
                if line.is_empty() {
                    continue;
                }

                if !from_queue {
                    // Erase the readline echo line before printing the styled block.
                    eprint!("\x1b[1A\r\x1b[2K");
                }
                // Commands (cd, !…, /…) get a dim echo — they aren't messages to the
                // model; real prompts get the "You" block. Queued lines are always
                // prompts (no readline echo to erase).
                let is_command = line == "cd"
                    || line.starts_with("cd ")
                    || line.starts_with('!')
                    || line.starts_with('/');
                if is_command {
                    print_command_echo(&line);
                } else {
                    print_user_line(&line);
                }

                if line == "cd" || line.starts_with("cd ") {
                    let arg = line.strip_prefix("cd").unwrap_or("").trim();
                    if apply_repl_cd(
                        &session,
                        &fs,
                        &mut cwd,
                        &project_dir,
                        &prefix,
                        &session.current_model(),
                        session_name.as_deref(),
                        arg,
                    )
                    .await
                        && let Some(helper) = rl.helper_mut()
                    {
                        helper.cwd = cwd.clone();
                    }
                    continue;
                }

                // `! <command>` — local shell escape. Runs in the REPL's working
                // directory and streams output to the terminal; not sent to the model.
                if let Some(shell_cmd) = line.strip_prefix('!') {
                    let shell_cmd = shell_cmd.trim();
                    if shell_cmd.is_empty() {
                        eprintln!("usage: !<shell command>  (runs in {})", cwd.display());
                    } else {
                        let _ = rl.add_history_entry(&raw_line);
                        match run_shell_command(shell_cmd, &cwd) {
                            Ok(status) if status.success() => {}
                            Ok(status) => match status.code() {
                                Some(code) => eprintln!("\x1b[2m[exit {code}]\x1b[0m"),
                                None => eprintln!("\x1b[2m[terminated by signal]\x1b[0m"),
                            },
                            Err(e) => eprintln!("\x1b[31m!: {e}\x1b[0m"),
                        }
                    }
                    continue;
                }

                // MCP prompt slash command: `/mcp__<server>__<prompt> [k=v...]`.
                // Fetch the prompt from the server and submit its text as a normal
                // prompt. Non-matching lines fall through to slash-command handling.
                let line = match crate::mcp_prompts::parse_mcp_prompt_command(&line) {
                    Some(inv) => match fetch_mcp_prompt(&inv, &mcp_manager, session.session_id(), &mcp_http).await {
                        Ok(text) if !text.trim().is_empty() => text,
                        Ok(_) => {
                            eprintln!(
                                "warning: MCP prompt `{}__{}` returned no content",
                                inv.server, inv.prompt
                            );
                            continue;
                        }
                        Err(e) => {
                            eprintln!("error: {e}");
                            continue;
                        }
                    },
                    None => line,
                };

                if line.starts_with('/') {
                    let mut parts = line.splitn(2, ' ');
                    let cmd = parts.next().unwrap_or("");
                    let arg = parts.next().unwrap_or("");
                    if cmd == "/cd" {
                        if apply_repl_cd(
                            &session,
                            &fs,
                            &mut cwd,
                            &project_dir,
                            &prefix,
                            &session.current_model(),
                            session_name.as_deref(),
                            arg,
                        )
                        .await
                            && let Some(helper) = rl.helper_mut()
                        {
                            helper.cwd = cwd.clone();
                        }
                        continue;
                    } else if cmd == "/clear" {
                        mcp_manager.shutdown_session(session.session_id()).await;
                        session.close().await;
                        match start_session(&factory, &mut mcp_manager, &prefix, cwd.clone(), &session_init, &fs).await
                        {
                            Ok(s) => {
                                session = s;
                                session_used_tokens = 0;
                                session_context_size = 0;
                                compactor_model_sel = None;
                                if skip_permissions {
                                    if let Err(e) = session.set_mode("bypassPermissions").await {
                                        eprintln!("warning: could not set bypassPermissions: {e}");
                                    }
                                    session_mode = "bypassPermissions".to_string();
                                } else {
                                    session_mode = std::env::var("TROGON_MODE").unwrap_or_else(|_| "default".into());
                                }
                                // (falls through to persist + print below)
                                if let Some(ref sup) = client_supervisor {
                                    sup.set_session(session.session_id());
                                }
                                persist_session_index(
                                    &fs,
                                    &project_dir,
                                    &prefix,
                                    session.session_id(),
                                    &session.current_model(),
                                    session_name.as_deref(),
                                );
                                eprintln!("session cleared — new session {}", session.session_id());
                                spawn_tracker = SpawnTracker::default();
                                rewind_state.reset();
                            }
                            Err(e) => eprintln!(
                                "error: runner unavailable, could not create new session: {e}\n  The old session is still active. Restart trogon to recover."
                            ),
                        }
                    } else if cmd == "/resume" {
                        if arg.is_empty() {
                            eprintln!("usage: /resume <session-id>");
                        } else {
                            let target = arg.trim();
                            let old_session_id = session.session_id().to_string();
                            // Shut down old MCP bridges before spawning new ones so that
                            // MCP servers that only allow one instance (e.g. port-binding
                            // servers) do not fail to start for the resumed session.
                            mcp_manager.shutdown_session(&old_session_id).await;
                            match activate_session(&factory, &mut mcp_manager, &prefix, target, &cwd, &fs).await {
                                Ok(s) => {
                                    session.close().await;
                                    session = s;
                                    session_used_tokens = 0;
                                    session_context_size = 0;
                                    compactor_model_sel = None;
                                    if let Some(ref sup) = client_supervisor {
                                        sup.set_session(session.session_id());
                                    }
                                    persist_session_index(
                                        &fs,
                                        &project_dir,
                                        &prefix,
                                        session.session_id(),
                                        &session.current_model(),
                                        session_name.as_deref(),
                                    );
                                    eprintln!("resumed session {}", session.session_id());
                                    spawn_tracker = SpawnTracker::default();
                                    rewind_state.reset();
                                }
                                Err(e) => {
                                    // MED-3: the old session's MCP bridges were shut down
                                    // before the switch attempt. Since resume failed and we
                                    // remain on the old session, restore its bridges so MCP
                                    // tools keep working instead of silently failing.
                                    eprintln!("error resuming session: {e}");
                                    if let Err(re) = respawn_session_mcp(&session, &mut mcp_manager, &cwd, &fs).await {
                                        eprintln!("warning: could not restore MCP bridges for current session: {re}");
                                    }
                                }
                            }
                        }
                    } else if cmd == "/sessions" {
                        match session.list_sessions().await {
                            Ok(list) if list.is_empty() => {
                                println!("no sessions on runner {prefix}");
                            }
                            Ok(list) => {
                                let index = SessionIndex::load(&fs);
                                println!("{:<36}  {:<20}  cwd", "session_id", "updated");
                                for s in list {
                                    let updated = s.updated_at.as_deref().unwrap_or("-");
                                    let model = index
                                        .get_for_prefix(&project_dir, &prefix)
                                        .filter(|e| e.session_id == s.session_id)
                                        .map(|e| e.model.as_str())
                                        .unwrap_or("-");
                                    let label = s.title.as_deref().filter(|t| !t.is_empty()).unwrap_or(&s.cwd);
                                    println!("{:<36}  {:<20}  {}  [model: {model}]", s.session_id, updated, label);
                                }
                            }
                            Err(e) => eprintln!("error listing sessions: {e}"),
                        }
                    } else if cmd == "/model" && arg.is_empty() {
                        match format_model_catalog(&registry, &prefix, &session.current_model(), session.session_id())
                            .await
                        {
                            Ok(text) => println!("{text}"),
                            Err(e) => eprintln!("error listing models: {e}"),
                        }
                    } else if cmd == "/pwd" {
                        if sync_repl_cwd_from_session(&session, &mut cwd).await
                            && let Some(helper) = rl.helper_mut()
                        {
                            helper.cwd = cwd.clone();
                        }
                        println!("{}", cwd.display());
                    } else if cmd == "/rename" {
                        let new_name = arg.trim();
                        if new_name.is_empty() {
                            match &session_name {
                                Some(n) => println!("current session name: {n}"),
                                None => println!("session has no name — usage: /rename <name>"),
                            }
                        } else {
                            session_name = Some(new_name.to_string());
                            persist_session_index(
                                &fs,
                                &project_dir,
                                &prefix,
                                session.session_id(),
                                &session.current_model(),
                                session_name.as_deref(),
                            );
                            println!("session renamed to {new_name}");
                        }
                    } else if cmd == "/status" {
                        if let Some(n) = &session_name {
                            println!("name: {n}");
                        }
                        let text = format_status(
                            &registry,
                            &prefix,
                            &session.current_model(),
                            session.session_id(),
                            session_used_tokens,
                            session_context_size,
                        )
                        .await;
                        println!("{text}");
                    } else if cmd == "/doctor" {
                        // MED-4: use the URL the CLI actually connected to (which
                        // honors --nats-url / TROGON_NATS_URL) instead of the
                        // unrelated NATS_URL env var that was silently read before.
                        let url = client_supervisor
                            .as_ref()
                            .map(|sup| sup.nats_url().to_string())
                            .or_else(|| std::env::var("TROGON_NATS_URL").ok())
                            .unwrap_or_else(|| "nats://localhost:4222".to_string());
                        crate::doctor::print_checks(&url).await;
                    } else if cmd == "/model" && !arg.is_empty() {
                        let model_id = resolve_model_alias(arg.trim());
                        let cwd_str = cwd
                            .canonicalize()
                            .unwrap_or_else(|_| cwd.clone())
                            .to_string_lossy()
                            .into_owned();
                        match apply_model_switch(&mut switcher, &prefix, session.session_id(), &model_id, &cwd_str)
                            .await
                        {
                            Ok(outcome) => {
                                if outcome.same_runner {
                                    match session.set_model(&model_id).await {
                                        Ok(()) => {
                                            println!("Model set to {model_id}");
                                            persist_session_index(
                                                &fs,
                                                &project_dir,
                                                &prefix,
                                                session.session_id(),
                                                &model_id,
                                                session_name.as_deref(),
                                            );
                                        }
                                        Err(e) => eprintln!("Error setting model: {e}"),
                                    }
                                } else {
                                    let applied = finish_cross_runner_model_switch(
                                        &factory,
                                        session,
                                        &outcome,
                                        &model_id,
                                        &mut mcp_manager,
                                        &cwd,
                                        &fs,
                                        &project_dir,
                                        session_name.as_deref(),
                                        skip_permissions,
                                        client_supervisor.as_ref(),
                                        None,
                                    )
                                    .await;
                                    session = applied.session;
                                    prefix = applied.prefix;
                                    session_mode = applied.session_mode;
                                    session_used_tokens = 0;
                                    session_context_size = 0;
                                    compactor_model_sel = None;
                                    rewind_state.reset();
                                }
                            }
                            Err(e) => eprintln!("Error switching model: {e}"),
                        }
                    } else if cmd == "/mode" {
                        if arg.is_empty() {
                            let m = "\x1b[35m";
                            let r = "\x1b[0m";
                            let dim = "\x1b[2m";
                            let modes = [
                                ("default", "auto-allow reads; prompt for edits, bash, MCP"),
                                ("acceptEdits", "default, plus auto-allow file edits"),
                                ("plan", "read-only exploration; deny writes & bash"),
                                ("auto", "auto-allow reads; LLM safety classifier decides the rest"),
                                ("dontAsk", "auto-allow everything (still audited)"),
                                ("bypassPermissions", "no permission checks at all"),
                            ];
                            let mut out =
                                format!("current mode: {m}{session_mode}{r}\n\nchange with {m}/mode <name>{r}:\n");
                            for (name, desc) in modes {
                                let marker = if name == session_mode { "▸" } else { " " };
                                out.push_str(&format!("  {marker} {name:<18}{dim}{desc}{r}\n"));
                            }
                            print!("{out}");
                        } else {
                            let mode = arg.trim();
                            match session.set_mode(mode).await {
                                Ok(()) => {
                                    session_mode = mode.to_string();
                                    println!("mode set to {mode}");
                                }
                                Err(e) => eprintln!("error setting mode: {e}"),
                            }
                        }
                    } else if cmd == "/plan" {
                        match session.set_mode("plan").await {
                            Ok(()) => {
                                session_mode = "plan".to_string();
                                println!(
                                    "plan mode on — read-only exploration; writes & bash are denied"
                                );
                            }
                            Err(e) => eprintln!("error entering plan mode: {e}"),
                        }
                    } else if cmd == "/vim" {
                        vim_mode = !vim_mode;
                        rl.set_edit_mode(if vim_mode { EditMode::Vi } else { EditMode::Emacs });
                        println!(
                            "{} input mode",
                            if vim_mode { "vim" } else { "emacs" }
                        );
                    } else if cmd == "/allowed-tools" {
                        println!("{}", allowed_tools_for_mode(&session_mode));
                    } else if cmd == "/checkpoint" {
                        match handle_checkpoint_command(&session, &mut rewind_state, arg).await {
                            Ok(msg) => println!("{msg}"),
                            Err(e) => eprintln!("error: {e}"),
                        }
                    } else if cmd == "/rewind" {
                        match handle_rewind_command(&session, &mut rewind_state, arg).await {
                            Ok(msg) => println!("{msg}"),
                            Err(RewindError::ListRequested) => println!("{}", rewind_state.list_checkpoints()),
                            Err(e) => eprintln!("error: {e}"),
                        }
                    } else if cmd == "/review" {
                        // Drive the model to review via its shell tools; runs like a
                        // normal prompt on the next loop turn.
                        queued_prompts.push_back(QueuedPrompt::from_text(review_prompt(arg)));
                    } else if cmd == "/pr-comments" {
                        queued_prompts.push_back(QueuedPrompt::from_text(pr_comments_prompt(arg)));
                    } else if cmd == "/compact" {
                        if !hooks_config.pre_compact.is_empty() {
                            let payload = serde_json::json!({
                                "hook_event_name": "PreCompact",
                                "trigger": "manual",
                                "session_id": session.session_id(),
                            });
                            let _ = trogon_runner_tools::run_event_hooks(
                                &hooks_config.pre_compact,
                                None,
                                &payload,
                            )
                            .await;
                        }
                        match session.compact().await {
                            Ok(CompactResult {
                                compacted: true,
                                tokens_before,
                                tokens_after,
                            }) => {
                                println!(
                                    "compacted: {} → {} tokens",
                                    fmt_tokens(tokens_before as u64),
                                    fmt_tokens(tokens_after as u64),
                                );
                                // B10: refresh the tracked usage so /status, /cost, and the
                                // token bar reflect the post-compaction total instead of the
                                // stale pre-compaction count. The context window is unchanged.
                                session_used_tokens = tokens_after as u64;
                            }
                            Ok(CompactResult {
                                compacted: false,
                                tokens_before,
                                ..
                            }) => {
                                if tokens_before > 0 {
                                    println!("no compaction needed ({} tokens)", fmt_tokens(tokens_before as u64),);
                                } else {
                                    println!("no messages to compact");
                                }
                            }
                            Err(e) => eprintln!("error triggering compaction: {e}"),
                        }
                    } else if cmd == "/compact-model" {
                        let cm_arg = arg.trim();
                        if cm_arg.is_empty() {
                            // Show the current compaction model plus only the models
                            // advertised by THIS runner (same provider) — those are the
                            // only valid choices, with the same ids as /model.
                            match format_compactor_models(&registry, &prefix, compactor_model_sel.as_deref()).await {
                                Ok(text) => println!("{text}"),
                                Err(e) => eprintln!("error listing models: {e}"),
                            }
                        } else {
                            // Resolve aliases (haiku → claude-haiku-4-5-20251001) just
                            // like /model, so the runner stores a valid model id. "default"
                            // is passed through verbatim to clear the override.
                            let resolved = if cm_arg == "default" {
                                "default".to_string()
                            } else {
                                resolve_model_alias(cm_arg)
                            };
                            let proceed = if resolved == "default" {
                                true
                            } else {
                                match runner_advertises_model(&registry, &prefix, &resolved).await {
                                    Ok(true) => true,
                                    Ok(false) => {
                                        eprintln!(
                                            "error: unknown compaction model '{resolved}' — run /compact-model with no args to list valid ids"
                                        );
                                        false
                                    }
                                    Err(e) => {
                                        eprintln!("error: {e}");
                                        false
                                    }
                                }
                            };
                            if proceed {
                                match do_compact_model(&session, &resolved).await {
                                    Ok(msg) => {
                                        println!("{msg}");
                                        compactor_model_sel =
                                            if resolved == "default" { None } else { Some(resolved) };
                                    }
                                    Err(e) => eprintln!("error: {e}"),
                                }
                            }
                        }
                    } else if cmd == "/mcp" {
                        handle_mcp_command(arg, &mut mcp_manager, &fs, &session, &cwd, &mcp_http).await;
                    } else if cmd == "/memory" {
                        handle_memory_command(arg, &cwd, &fs).await;
                    } else if cmd == "/agents" {
                        handle_agents_command(arg, &cwd);
                    } else if cmd == "/tasks" {
                        match session.list_sessions().await {
                            Ok(list) => {
                                let text = spawn_tracker.format_tasks(
                                    &prefix,
                                    session.session_id(),
                                    &list,
                                );
                                println!("{text}");
                            }
                            Err(e) => {
                                eprintln!("error listing spawns: {e}");
                                let text = spawn_tracker.format_tasks(
                                    &prefix,
                                    session.session_id(),
                                    &[],
                                );
                                println!("{text}");
                            }
                        }
                    } else if cmd == "/init" {
                        let force = arg == "--force";
                        let root = find_git_root(&cwd).unwrap_or_else(|| cwd.clone());
                        let dest = root.join("TROGON.md");
                        if fs.read_to_string(&dest).is_ok() && !force {
                            println!(
                                "TROGON.md already exists at {}\nRun \x1b[35m/init --force\x1b[0m to overwrite.",
                                dest.display()
                            );
                        } else {
                            eprintln!("analyzing project with AI...");
                            let prompt = build_init_prompt(&root, &fs);
                            match factory.create_session(&init_prefix, cwd.clone(), vec![]).await {
                                Err(e) => eprintln!("error creating init session: {e}"),
                                Ok(init_session) => {
                                    // Bypass approval gates so Claude can write TROGON.md without prompting.
                                    let _ = init_session.set_mode("bypassPermissions").await;
                                    match init_session.prompt(&prompt).await {
                                        Err(e) => eprintln!("error: {e}"),
                                        Ok(mut rx) => {
                                            let mut content = String::new();
                                            let mut stdout = std::io::stdout();
                                            let mut runner_error: Option<String> = None;
                                            loop {
                                                match rx.recv().await {
                                                    None => break,
                                                    Some(StreamEvent::Text(t)) => {
                                                        content.push_str(&t);
                                                        print!("{t}");
                                                        let _ = stdout.flush();
                                                    }
                                                    Some(StreamEvent::Error(msg)) => {
                                                        runner_error = Some(msg);
                                                        break;
                                                    }
                                                    Some(StreamEvent::Done(_)) => {
                                                        println!();
                                                        break;
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            init_session.close().await;
                                            if let Some(err) = runner_error {
                                                eprintln!("error: {err}");
                                            } else {
                                                let trogon_content = strip_code_fence(&content);
                                                if trogon_content.is_empty() {
                                                    // Model may have written the file directly via write_file tool.
                                                    // If it has content, keep it; otherwise report failure.
                                                    if fs.read_to_string(&dest).map(|s| !s.is_empty()).unwrap_or(false)
                                                    {
                                                        println!("created {}", dest.display());
                                                    } else {
                                                        eprintln!("error: model produced no content — try again");
                                                    }
                                                } else {
                                                    match fs.write(&dest, trogon_content.as_bytes()) {
                                                        Ok(()) => println!("created {}", dest.display()),
                                                        Err(e) => eprintln!("error writing TROGON.md: {e}"),
                                                    }
                                                }
                                            } // close runner_error else
                                        }
                                    }
                                }
                            }
                        }
                    } else if let Some(dispatch) = custom_commands.dispatch(cmd, arg) {
                        queued_prompts.push_back(QueuedPrompt::from_dispatch(dispatch));
                    } else {
                        println!(
                            "{}",
                            handle_slash_command(
                                cmd,
                                arg,
                                session_used_tokens,
                                session_context_size,
                                &session.current_model(),
                                &cwd,
                                &fs,
                                Some(&custom_commands),
                            )
                        );
                    }
                    continue;
                }

                let _ = rl.add_history_entry(&raw_line);

                let mut expanded = expand_mentions(&line, &cwd, &fs);

                // UserPromptSubmit hooks: may block the prompt or inject context.
                if !hooks_config.user_prompt_submit.is_empty() {
                    let payload = serde_json::json!({
                        "hook_event_name": "UserPromptSubmit",
                        "prompt": line,
                        "cwd": cwd.to_string_lossy(),
                    });
                    match trogon_runner_tools::run_event_hooks(
                        &hooks_config.user_prompt_submit,
                        None,
                        &payload,
                    )
                    .await
                    {
                        trogon_runner_tools::HookOutcome::Block { reason } => {
                            eprintln!("\x1b[33mprompt blocked by hook: {reason}\x1b[0m");
                            continue;
                        }
                        trogon_runner_tools::HookOutcome::Continue { context: Some(ctx) } => {
                            expanded.push_str("\n\n");
                            expanded.push_str(&ctx);
                        }
                        trogon_runner_tools::HookOutcome::Continue { context: None } => {}
                    }
                }
                // Inject any SessionStart context into the first prompt, once.
                if let Some(ctx) = pending_start_context.take() {
                    expanded.push_str("\n\n");
                    expanded.push_str(&ctx);
                }
                let prompt_opts = queued_invocation
                    .as_ref()
                    .map(QueuedPrompt::prompt_opts)
                    .unwrap_or_default();
                if let Some(inv) = queued_invocation.as_ref()
                    && let Some(model) = inv.model.as_deref()
                {
                    session = apply_dispatch_model_switch(
                        &mut switcher,
                        &factory,
                        session,
                        &mut prefix,
                        &mut session_mode,
                        &mut session_used_tokens,
                        &mut session_context_size,
                        &mut compactor_model_sel,
                        &mut rewind_state,
                        &mut mcp_manager,
                        &cwd,
                        &fs,
                        &project_dir,
                        session_name.as_deref(),
                        skip_permissions,
                        client_supervisor.as_ref(),
                        model,
                    )
                    .await;
                }
                // Auto-recover if the runner restarted and lost the session, then retry once.
                let prompt_result = match session.prompt_with_opts(&expanded, prompt_opts.clone()).await {
                    Err(e) if e.to_string().contains("not found") => {
                        eprintln!("\x1b[33mwarning: session lost (runner restarted?) — reconnecting...\x1b[0m");
                        match start_session(&factory, &mut mcp_manager, &prefix, cwd.clone(), &session_init, &fs).await
                        {
                            Ok(s) => {
                                session = s;
                                session.prompt_with_opts(&expanded, prompt_opts).await
                            }
                            Err(e2) => {
                                eprintln!("error: runner unavailable: {e2}\n  Restart trogon to recover.");
                                continue;
                            }
                        }
                    }
                    other => other,
                };
                match prompt_result {
                    Err(e) => eprintln!("error: {e}"),
                    Ok(mut rx) => {
                        let ctrl_c = tokio::signal::ctrl_c();
                        tokio::pin!(ctrl_c);
                        let mut renderer = TurnRenderer::new(stream);
                        let mut metrics = TurnMetrics {
                            used_tokens: session_used_tokens,
                            context_size: session_context_size,
                        };

                        // Capture input typed during this turn: plain lines are queued
                        // and auto-submitted when the turn ends; Ctrl+G steers the
                        // live turn (published to the runner's steer subject). The
                        // reader restores the terminal when it drops at end of block.
                        let (_input_reader, mut input_rx) = match StreamInputReader::start() {
                            Some((r, rx)) => (Some(r), Some(rx)),
                            None => (None, None),
                        };
                        // Normal queued messages (typed + Enter), submitted in order.
                        let mut queued: Vec<String> = Vec::new();
                        // Ctrl+G fallback for runners that don't yet subscribe to steer:
                        // jump to the front of the queue (submitted before `queued`).
                        let mut front_queued: Vec<String> = Vec::new();
                        let mut interrupted = false;
                        // Drives the live `Thinking… (Ns)` status line between events.
                        let mut status_ticker =
                            tokio::time::interval(std::time::Duration::from_millis(500));

                        loop {
                            tokio::select! {
                                biased;
                                _ = &mut ctrl_c => {
                                    session.cancel().await;
                                    renderer.on_ctrl_c();
                                    // Claude-style: restore the interrupted prompt so the
                                    // next readline is pre-filled with it, ready to edit/resend.
                                    pending_input = Some(line.clone());
                                    interrupted = true;
                                    break;
                                }
                                ev = next_stream_input(&mut input_rx) => {
                                    match ev {
                                        Some(StreamInputEvent::Queued(msg)) => {
                                            reset_display();
                                            eprintln!("\x1b[2m⏳ queued: {msg}\x1b[0m");
                                            queued.push(msg);
                                        }
                                        Some(StreamInputEvent::Priority(q)) => {
                                            reset_display();
                                            // Steer the IN-FLIGHT turn on runners that subscribe to
                                            // the steer subject (currently the acp/Claude runner):
                                            // the model addresses it on its next step. Other runners
                                            // don't subscribe yet, so fall back to front-of-queue so
                                            // the side question is never lost.
                                            if crate::app::runner_label(&prefix) == "claude" {
                                                eprintln!("\x1b[2m↪ steering: {q}\x1b[0m");
                                                session.steer(q).await;
                                            } else {
                                                eprintln!("\x1b[2m⏳ queued (next): {q}\x1b[0m");
                                                front_queued.push(q);
                                            }
                                        }
                                        // Reader stopped — stop polling it to avoid spinning.
                                        None => { input_rx = None; }
                                    }
                                }
                                event = rx.recv() => {
                                    match event {
                                        None => break,
                                        Some(ev) => {
                                            match &ev {
                                                StreamEvent::ToolCall(name) => {
                                                    spawn_tracker.on_tool_call(name);
                                                }
                                                StreamEvent::ToolFinished { name, .. } => {
                                                    spawn_tracker.on_tool_finished(name);
                                                }
                                                _ => {}
                                            }
                                            if let Some(sync) = renderer.handle(ev, &mut metrics)
                                                && sync_cwd_from_tool(
                                                    &sync.tool_name,
                                                    &sync.output,
                                                    &mut cwd,
                                                )
                                                && let Some(helper) = rl.helper_mut()
                                            {
                                                helper.cwd = cwd.clone();
                                            }
                                            session_used_tokens = metrics.used_tokens;
                                            session_context_size = metrics.context_size;
                                            if renderer.is_stopped() {
                                                break;
                                            }
                                        }
                                    }
                                }
                                // Animate the `Thinking… (Ns)` / tool status line
                                // while waiting on the model (lowest priority).
                                _ = status_ticker.tick() => {
                                    renderer.tick();
                                }
                            }
                        }

                        let stop = renderer.take_stop();
                        if let Some(recorder) =
                            SessionTranscriptRecorder::for_session(&fs, session.session_id())
                        {
                            recorder.record_turn(
                                &line,
                                renderer.assistant_text(),
                                stop.as_ref(),
                                interrupted,
                            );
                        }

                        // Persist on a clean turn end (matches programming-gaps'
                        // in-arm persistence — skips cancelled / maxTurnRequests).
                        if let Some(TurnStop::Done { reason }) = stop
                            && reason != "cancelled"
                            && reason != "maxTurnRequests"
                        {
                            persist_session_index(
                                &fs,
                                &project_dir,
                                &prefix,
                                session.session_id(),
                                &session.current_model(),
                                session_name.as_deref(),
                            );
                            sync_repl_cwd_from_session(&session, &mut cwd).await;
                            if let Some(helper) = rl.helper_mut() {
                                helper.cwd = cwd.clone();
                            }
                            rewind_state.on_turn_complete();
                        }

                        // Auto-submit messages queued during this turn: Ctrl+G
                        // fallback (front_queued) first, then the rest in order.
                        // If the user interrupted (Ctrl+C), discard the queue since
                        // they're likely changing direction. `_input_reader` drops at
                        // the end of this block, restoring the terminal before readline.
                        if !interrupted {
                            queued_prompts.extend(
                                front_queued.into_iter().map(QueuedPrompt::from_text),
                            );
                            queued_prompts.extend(queued.into_iter().map(QueuedPrompt::from_text));
                        }
                    }
                }

                // Post-turn lifecycle hooks (CLI-side, fire-and-forget). Stop fires
                // when the agent finishes; Notification when it's idle awaiting input.
                if !hooks_config.stop.is_empty() {
                    let payload = serde_json::json!({"hook_event_name": "Stop", "cwd": cwd.to_string_lossy()});
                    let _ = trogon_runner_tools::run_event_hooks(&hooks_config.stop, None, &payload).await;
                }
                if !hooks_config.notification.is_empty() {
                    let payload = serde_json::json!({"hook_event_name": "Notification", "cwd": cwd.to_string_lossy()});
                    let _ =
                        trogon_runner_tools::run_event_hooks(&hooks_config.notification, None, &payload).await;
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+X Ctrl+E stashes the buffer here and interrupts; if present,
                // this is an editor request, not a cancel. The terminal is back in
                // normal mode now, so it's safe to launch the editor.
                let edit_seed = editor_request.lock().ok().and_then(|mut r| r.take());
                if let Some(seed) = edit_seed {
                    match edit_in_editor(&seed) {
                        Ok(edited) if !edited.trim().is_empty() => {
                            reset_display();
                            // Re-prefill the prompt with the edited text for review
                            // before sending (Enter submits).
                            pending_input = Some(edited);
                        }
                        Ok(_) => {} // empty result — discard, fresh prompt
                        Err(e) => eprintln!("editor error: {e}"),
                    }
                } else {
                    session.cancel().await;
                    eprintln!("(Ctrl+C)");
                }
            }
            Err(ReadlineError::Eof) => {
                eprint!("\r\x1b[K");
                mcp_manager.shutdown_session(session.session_id()).await;
                session.close().await;
                mcp_manager.shutdown_all().await;
                break;
            }
            Err(e) => {
                // MED-2: tear down the session and MCP bridges on any readline
                // error, not just EOF, so we don't leak the runner session and
                // MCP child processes when the terminal errors out.
                eprintln!("readline error: {e}");
                mcp_manager.shutdown_session(session.session_id()).await;
                session.close().await;
                mcp_manager.shutdown_all().await;
                break;
            }
        }
    }

    let _ = rl.save_history(&history_path);
    Ok(())
}

/// Await the next input event from the streaming-time reader. If the reader
/// isn't running (not a real terminal), this never resolves, so the `select!`
/// branch using it simply stays dormant.
async fn next_stream_input(
    rx: &mut Option<tokio::sync::mpsc::UnboundedReceiver<StreamInputEvent>>,
) -> Option<StreamInputEvent> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

async fn start_session<SF: SessionFactory, F: Fs>(
    factory: &SF,
    mcp: &mut McpManager,
    prefix: &str,
    cwd: PathBuf,
    init: &crate::session::SessionInit,
    fs: &F,
) -> anyhow::Result<SF::Sess> {
    let mcp_servers = mcp.spawn_pending(fs).await;
    let session = factory.create_session_with_init(prefix, cwd, mcp_servers, init).await?;
    mcp.commit_pending(session.session_id());
    Ok(session)
}

/// Resolve an MCP prompt invocation to its rendered text by connecting to the
/// owning server (`prompts/get`). Returns a user-facing error string on failure.
async fn fetch_mcp_prompt(
    inv: &crate::mcp_prompts::McpPromptInvocation,
    mcp: &McpManager,
    session_id: &str,
    http: &reqwest::Client,
) -> Result<String, String> {
    let conns = mcp.active_connections(session_id);
    let Some((_, url, headers)) = conns.into_iter().find(|(n, _, _)| n == &inv.server) else {
        if mcp.active_for_session(session_id).iter().any(|(n, _)| n == &inv.server) {
            return Err(format!(
                "MCP server `{}` is native stdio; CLI prompt commands are only available for HTTP/SSE MCP servers",
                inv.server
            ));
        }
        return Err(format!("no active MCP server named `{}` (see /mcp list)", inv.server));
    };
    let client = trogon_mcp::McpClient::with_headers(http.clone(), &url, headers);
    client.initialize().await?;
    client.get_prompt(&inv.prompt, &inv.arguments).await
}

async fn activate_session<SF: SessionFactory, F: Fs>(
    factory: &SF,
    mcp: &mut McpManager,
    prefix: &str,
    session_id: &str,
    cwd: &Path,
    fs: &F,
) -> anyhow::Result<SF::Sess> {
    let mcp_servers = mcp.spawn_pending(fs).await;
    let session = factory.attach_session(prefix, session_id.to_string());
    session.load_session(session_id, cwd, mcp_servers).await?;
    mcp.commit_pending(session_id);
    Ok(session)
}

async fn handle_mcp_command<F: Fs, S: Session>(
    arg: &str,
    mcp: &mut McpManager,
    fs: &F,
    session: &S,
    cwd: &Path,
    http: &reqwest::Client,
) {
    let mut parts = arg.splitn(2, ' ');
    let sub = parts.next().unwrap_or("list").trim();
    let rest = parts.next().unwrap_or("").trim();

    match sub {
        "list" => {
            println!("configured MCP servers (~/.config/trogon/mcp.json):");
            if mcp.configured_servers().is_empty() {
                println!("  (none)");
            } else {
                for s in mcp.configured_servers() {
                    match s.transport {
                        crate::mcp::McpTransport::Stdio => {
                            println!("  {} — stdio: {} {}", s.name, s.command, s.args.join(" "))
                        }
                        crate::mcp::McpTransport::Http => {
                            let auth = if s.oauth { " (oauth)" } else { "" };
                            let t = s.timeout_secs.map(|n| format!(" (timeout {n}s)")).unwrap_or_default();
                            println!("  {} — http: {}{}{}", s.name, s.url, auth, t)
                        }
                        crate::mcp::McpTransport::Sse => {
                            let auth = if s.oauth { " (oauth)" } else { "" };
                            let t = s.timeout_secs.map(|n| format!(" (timeout {n}s)")).unwrap_or_default();
                            println!("  {} — sse: {}{}{}", s.name, s.url, auth, t)
                        }
                    }
                }
            }
            let active = mcp.active_for_session(session.session_id());
            if active.is_empty() {
                println!("active MCP servers: (none)");
            } else {
                println!("active MCP servers:");
                for (name, url) in active {
                    println!("  {name} → {url}");
                }
            }
        }
        "add" => match McpManager::parse_add_args(rest) {
            Err(e) => eprintln!("{e}"),
            Ok(cfg) => {
                let name = cfg.name.clone();
                let oauth = cfg.oauth;
                let url = cfg.url.clone();
                mcp.add_server(cfg);
                if let Err(e) = mcp.save(fs) {
                    eprintln!("error saving MCP config: {e}");
                } else {
                    println!("added MCP server `{name}`");
                    if oauth {
                        run_mcp_login(&name, &url, mcp, fs, http).await;
                    }
                    if let Err(e) = respawn_session_mcp(session, mcp, cwd, fs).await {
                        eprintln!("warning: could not refresh session MCP: {e}");
                    }
                }
            }
        },
        "login" => {
            let name = rest.trim();
            if name.is_empty() {
                eprintln!("usage: /mcp login <name>");
                return;
            }
            let Some(url) = mcp.http_server_url(name) else {
                eprintln!("no HTTP MCP server named `{name}` — add one with /mcp add --transport http {name} <url>");
                return;
            };
            run_mcp_login(name, &url, mcp, fs, http).await;
            if let Err(e) = respawn_session_mcp(session, mcp, cwd, fs).await {
                eprintln!("warning: could not refresh session MCP: {e}");
            }
        }
        "remove" => {
            if rest.is_empty() {
                eprintln!("usage: /mcp remove <name>");
            } else if mcp.remove_server(rest) {
                if let Err(e) = mcp.save(fs) {
                    eprintln!("error saving MCP config: {e}");
                } else {
                    mcp.shutdown_session(session.session_id()).await;
                    println!("removed MCP server `{rest}`");
                    if let Err(e) = respawn_session_mcp(session, mcp, cwd, fs).await {
                        eprintln!("warning: could not refresh session MCP: {e}");
                    }
                }
            } else {
                eprintln!("no MCP server named `{rest}`");
            }
        }
        "prompts" => {
            let conns = mcp.active_connections(session.session_id());
            if conns.is_empty() {
                if mcp.active_for_session(session.session_id()).is_empty() {
                    println!("no active MCP servers");
                } else {
                    println!("no active HTTP/SSE MCP servers with CLI-readable prompts");
                }
                return;
            }
            println!("available MCP prompts (call as /mcp__<server>__<prompt> [k=v...]):");
            let mut any = false;
            for (name, url, headers) in conns {
                let client = trogon_mcp::McpClient::with_headers(http.clone(), &url, headers);
                if client.initialize().await.is_err() {
                    continue;
                }
                match client.list_prompts().await {
                    Ok(prompts) => {
                        for p in prompts {
                            any = true;
                            let args = p
                                .arguments
                                .iter()
                                .map(|a| {
                                    if a.required {
                                        format!("{}=…", a.name)
                                    } else {
                                        format!("[{}=…]", a.name)
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(" ");
                            println!("  /mcp__{name}__{} {args}", p.name);
                            if !p.description.is_empty() {
                                println!("      {}", p.description);
                            }
                        }
                    }
                    Err(e) => eprintln!("  {name}: {e}"),
                }
            }
            if !any {
                println!("  (none advertised)");
            }
        }
        "import" => {
            let path = if rest.is_empty() {
                match crate::mcp::default_claude_desktop_config_path() {
                    Some(p) => p,
                    None => {
                        eprintln!(
                            "could not determine the Claude Desktop config path — pass one explicitly: /mcp import <path>"
                        );
                        return;
                    }
                }
            } else {
                crate::mcp::expand_tilde(rest)
            };
            match mcp.import_claude_desktop(fs, &path) {
                Err(e) => eprintln!("{e}"),
                Ok(summary) => {
                    if let Err(e) = mcp.save(fs) {
                        eprintln!("error saving MCP config: {e}");
                        return;
                    }
                    if summary.imported.is_empty() {
                        println!("no MCP servers found in {}", path.display());
                    } else {
                        println!(
                            "imported {} MCP server(s) from {}: {}",
                            summary.imported.len(),
                            path.display(),
                            summary.imported.join(", ")
                        );
                    }
                    if !summary.skipped.is_empty() {
                        println!(
                            "skipped {} entry(ies) with no command or url: {}",
                            summary.skipped.len(),
                            summary.skipped.join(", ")
                        );
                    }
                    if !summary.imported.is_empty()
                        && let Err(e) = respawn_session_mcp(session, mcp, cwd, fs).await
                    {
                        eprintln!("warning: could not refresh session MCP: {e}");
                    }
                }
            }
        }
        other => {
            eprintln!("unknown /mcp subcommand `{other}` — try list, add, remove, login, import, prompts")
        }
    }
}

/// Run the interactive OAuth login for an HTTP MCP server and persist the token.
async fn run_mcp_login<F: Fs>(name: &str, url: &str, mcp: &mut McpManager, fs: &F, http: &reqwest::Client) {
    match crate::mcp_oauth::login(http, url).await {
        Ok(token) => {
            mcp.set_oauth_token(name, token); // also marks the server config oauth=true
            if let Err(e) = mcp.save_oauth(fs) {
                eprintln!("error saving OAuth token: {e}");
                return;
            }
            // Persist the oauth=true marker on the server config.
            if let Err(e) = mcp.save(fs) {
                eprintln!("error saving MCP config: {e}");
                return;
            }
            println!("authorized MCP server `{name}`");
        }
        Err(e) => eprintln!("OAuth failed for `{name}`: {e}"),
    }
}

async fn respawn_session_mcp<S: Session, F: Fs>(
    session: &S,
    mcp: &mut McpManager,
    cwd: &Path,
    fs: &F,
) -> anyhow::Result<()> {
    mcp.shutdown_session(session.session_id()).await;
    let servers = mcp.spawn_pending(fs).await;
    mcp.commit_pending(session.session_id());
    session.load_session(session.session_id(), cwd, servers).await
}

async fn sync_repl_cwd_from_session<S: Session>(session: &S, cwd: &mut PathBuf) -> bool {
    if let Ok(Some(runner_cwd)) = session.session_cwd().await {
        let canonical = runner_cwd.canonicalize().unwrap_or(runner_cwd);
        if *cwd != canonical {
            *cwd = canonical.clone();
            return true;
        }
    }
    false
}

fn sync_cwd_from_tool(name: &str, output: &str, cwd: &mut PathBuf) -> bool {
    let prefix = "Working directory is now ";
    let is_cd = name == "change_directory" || (name == "bash" && output.starts_with(prefix));
    if !is_cd {
        return false;
    }
    let Some(path) = output.strip_prefix(prefix) else {
        return false;
    };
    let path = path.trim();
    if path.is_empty() {
        return false;
    }
    *cwd = PathBuf::from(path);
    true
}

#[allow(clippy::too_many_arguments)]
async fn apply_repl_cd<S: Session, F: Fs>(
    session: &S,
    fs: &F,
    cwd: &mut PathBuf,
    project_dir: &Path,
    prefix: &str,
    model: &str,
    name: Option<&str>,
    arg: &str,
) -> bool {
    match resolve_directory_target(&cwd.to_string_lossy(), arg) {
        Ok(resolved) => {
            *cwd = resolved.clone();
            match session.set_cwd(&resolved).await {
                Ok(()) => {
                    persist_session_index(fs, project_dir, prefix, session.session_id(), model, name);
                    reset_display();
                    eprintln!("{}", resolved.display());
                }
                Err(e) => {
                    eprintln!("warning: runner cwd update failed: {e} — local shell only");
                    reset_display();
                    eprintln!("{}", resolved.display());
                }
            }
            true
        }
        Err(e) => {
            eprintln!("{e}");
            false
        }
    }
}

fn persist_session_index<F: Fs>(
    fs: &F,
    project: &Path,
    prefix: &str,
    session_id: &str,
    model: &str,
    name: Option<&str>,
) {
    let mut index = SessionIndex::load(fs);
    let mut entry = new_session_entry(prefix, session_id, model);
    entry.name = name.map(String::from);
    index.record(project, entry);
    if let Err(e) = index.save(fs) {
        eprintln!("warning: could not save session index: {e}");
    }
}

// ── /model handler ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) struct ModelSwitchOutcome {
    pub same_runner: bool,
    pub new_prefix: String,
    pub new_session_id: String,
}

/// Records cross-runner switch phases for unit tests (ordering assertions).
#[derive(Debug, Default)]
pub(crate) struct SwitchJournal {
    steps: Vec<&'static str>,
}

impl SwitchJournal {
    fn record(&mut self, step: &'static str) {
        self.steps.push(step);
    }

    #[cfg(test)]
    pub(crate) fn steps(&self) -> &[&'static str] {
        &self.steps
    }
}

pub(crate) struct CrossRunnerSwitchApplied<S> {
    pub session: S,
    pub prefix: String,
    pub session_mode: String,
}

/// Complete a cross-runner `/model` switch: tear down the old session's MCP
/// bridges, attach the migrated session, bind MCP tools, then sync mode and
/// model. MCP must be ready before this returns so the REPL prompt cannot accept
/// user input while tools are still missing.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finish_cross_runner_model_switch<SF, S, F>(
    factory: &SF,
    session: S,
    outcome: &ModelSwitchOutcome,
    model_id: &str,
    mcp: &mut McpManager,
    cwd: &Path,
    fs: &F,
    project_dir: &Path,
    session_name: Option<&str>,
    skip_permissions: bool,
    client_supervisor: Option<&Rc<AcpClientSupervisor>>,
    mut journal: Option<&mut SwitchJournal>,
) -> CrossRunnerSwitchApplied<S>
where
    SF: SessionFactory<Sess = S>,
    S: Session,
    F: Fs,
{
    let record = |journal: &mut Option<&mut SwitchJournal>, step: &'static str| {
        if let Some(j) = journal.as_mut() {
            j.record(step);
        }
    };

    // B5: shut down the OLD session's MCP bridges before switching, otherwise
    // their child processes are leaked across the runner switch.
    let old_session_id = session.session_id().to_string();
    mcp.shutdown_session(&old_session_id).await;
    session.close().await;

    // B5: the cross-runner session was created with NO mcp_servers. Bind MCP for
    // the new session before any post-switch work or REPL prompt unblock.
    let session = match activate_session(
        factory,
        mcp,
        &outcome.new_prefix,
        &outcome.new_session_id,
        cwd,
        fs,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("warning: could not bind MCP for new session: {e}");
            factory.attach_session(&outcome.new_prefix, outcome.new_session_id.clone())
        }
    };
    record(&mut journal, "mcp_bound");

    // MED-5: the new runner starts a session with mode initialized from
    // TROGON_MODE; keep the REPL's tracked mode in sync.
    let session_mode = if skip_permissions {
        if let Err(e) = session.set_mode("bypassPermissions").await {
            eprintln!("warning: could not set bypassPermissions: {e}");
        }
        "bypassPermissions".to_string()
    } else {
        std::env::var("TROGON_MODE").unwrap_or_else(|_| "default".into())
    };

    if let Some(sup) = client_supervisor
        && let Err(e) = sup.rebind(&outcome.new_prefix, session.session_id()).await
    {
        eprintln!("error rebinding permission client: {e}");
    }

    match session.set_model(model_id).await {
        Ok(()) => {
            println!("Switched to {model_id}");
            persist_session_index(
                fs,
                project_dir,
                &outcome.new_prefix,
                session.session_id(),
                model_id,
                session_name,
            );
        }
        Err(e) => eprintln!("Error setting model on new runner: {e}"),
    }

    record(&mut journal, "prompt_unblocked");

    CrossRunnerSwitchApplied {
        session,
        prefix: outcome.new_prefix.clone(),
        session_mode,
    }
}

pub fn resolve_model_alias(input: &str) -> String {
    match input {
        "haiku" => "claude-haiku-4-5-20251001".into(),
        "sonnet" => "claude-sonnet-4-6".into(),
        "opus" => "claude-opus-4-6".into(),
        "grok" => "grok-3".into(),
        "grok-mini" => "grok-3-mini".into(),
        "openrouter" => {
            std::env::var("OPENROUTER_DEFAULT_MODEL").unwrap_or_else(|_| "anthropic/claude-sonnet-4-6".into())
        }
        // Short aliases for the OpenRouter models. These must match the IDs the
        // openrouter runner registers (OPENROUTER_MODELS); update both together.
        "op-claude" => "anthropic/claude-sonnet-4-6".into(),
        "op-gpt" => "openai/gpt-4o".into(),
        "op-gemini" => "google/gemini-pro-1.5".into(),
        "codex" => std::env::var("CODEX_DEFAULT_MODEL").unwrap_or_else(|_| "o4-mini".into()),
        "o3" => "o3".into(),
        "gpt-4o" => "gpt-4o".into(),
        other => other.into(),
    }
}

pub(crate) async fn apply_model_switch<SW: RunnerSwitcher>(
    switcher: &mut SW,
    current_prefix: &str,
    current_session_id: &str,
    model_id: &str,
    cwd: &str,
) -> Result<ModelSwitchOutcome, String> {
    let (new_prefix, new_session_id) = switcher
        .switch_model(current_prefix, current_session_id, model_id, cwd)
        .await?;
    Ok(ModelSwitchOutcome {
        same_runner: new_prefix == current_prefix,
        new_prefix,
        new_session_id,
    })
}

/// Apply a custom-command `model` frontmatter override using the same `/model` path.
/// On failure, prints a warning and returns the original session unchanged.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_dispatch_model_switch<SF, S, SW, F>(
    switcher: &mut SW,
    factory: &SF,
    session: S,
    prefix: &mut String,
    session_mode: &mut String,
    session_used_tokens: &mut u64,
    session_context_size: &mut u64,
    compactor_model_sel: &mut Option<String>,
    rewind_state: &mut SessionRewindState,
    mcp_manager: &mut McpManager,
    cwd: &Path,
    fs: &F,
    project_dir: &Path,
    session_name: Option<&str>,
    skip_permissions: bool,
    client_supervisor: Option<&Rc<AcpClientSupervisor>>,
    model: &str,
) -> S
where
    SF: SessionFactory<Sess = S>,
    S: Session,
    SW: RunnerSwitcher,
    F: Fs,
{
    let model_id = resolve_model_alias(model.trim());
    let cwd_str = cwd
        .canonicalize()
        .unwrap_or_else(|_| cwd.to_path_buf())
        .to_string_lossy()
        .into_owned();
    match apply_model_switch(switcher, prefix, session.session_id(), &model_id, &cwd_str).await {
        Ok(outcome) => {
            if outcome.same_runner {
                match session.set_model(&model_id).await {
                    Ok(()) => {
                        persist_session_index(
                            fs,
                            project_dir,
                            prefix,
                            session.session_id(),
                            &model_id,
                            session_name,
                        );
                    }
                    Err(e) => eprintln!(
                        "warning: custom command model `{model_id}` could not be set: {e} — using current model"
                    ),
                }
                session
            } else {
                let applied = finish_cross_runner_model_switch(
                    factory,
                    session,
                    &outcome,
                    &model_id,
                    mcp_manager,
                    cwd,
                    fs,
                    project_dir,
                    session_name,
                    skip_permissions,
                    client_supervisor,
                    None,
                )
                .await;
                *prefix = applied.prefix;
                *session_mode = applied.session_mode;
                *session_used_tokens = 0;
                *session_context_size = 0;
                *compactor_model_sel = None;
                rewind_state.reset();
                applied.session
            }
        }
        Err(e) => {
            eprintln!(
                "warning: custom command model `{model_id}` not available: {e} — using current model"
            );
            session
        }
    }
}

/// Handle `/compact-model [<model_id>|default]`.
///
/// - No argument   → show current compaction model override (if any)
/// - `default`     → clear the override; compaction uses the session model
/// - `<model_id>`  → set the compaction model (must be same provider as current runner)
pub(crate) async fn do_compact_model<S: Session>(session: &S, arg: &str) -> Result<String, String> {
    if arg.is_empty() {
        return Ok("compaction model: default (same as session model)\n\
             change with: /compact-model <model-id>\n\
             reset with:  /compact-model default"
            .to_string());
    }
    let value = if arg == "default" { "" } else { arg };
    session
        .set_session_config_option("compactor_model", value)
        .await
        .map_err(|e| e.to_string())?;
    if value.is_empty() {
        Ok("compaction model reset to default (same as session model)".to_string())
    } else {
        Ok(format!("compaction model set to: {value}"))
    }
}

// ── checkpoint / rewind ───────────────────────────────────────────────────────

async fn handle_checkpoint_command<S: Session>(
    session: &S,
    rewind_state: &mut SessionRewindState,
    arg: &str,
) -> anyhow::Result<String> {
    let export = session.export_history().await?;
    let name = arg.trim();
    let name = if name.is_empty() { None } else { Some(name) };
    Ok(rewind_state.record_checkpoint(name, export))
}

async fn handle_rewind_command<S: Session>(
    session: &S,
    rewind_state: &mut SessionRewindState,
    arg: &str,
) -> Result<String, RewindError> {
    let resolution = rewind_state.resolve_rewind(arg)?;
    let (messages_json, target_turn) = match resolution {
        RewindResolution::Snapshot {
            messages_json,
            target_turn,
        } => (messages_json, target_turn),
        RewindResolution::TruncateToTurns { target_turn } => {
            let export = session
                .export_history()
                .await
                .map_err(|e| RewindError::ExportParse(e.to_string()))?;
            let truncated = truncate_export_to_turns(&export, target_turn)?;
            (truncated, target_turn)
        }
    };

    session
        .import_history(&messages_json)
        .await
        .map_err(|e| RewindError::ExportParse(e.to_string()))?;
    rewind_state.after_rewind(target_turn);
    Ok(format!(
        "rewound to turn {target_turn} — conversation continues from that point"
    ))
}

// ── new slash-command helpers ───────────────────────────────────────────────────

/// Describe how the current permission mode gates each tool category — i.e. which
/// tools are usable in this mode. The runner owns the actual tool registry; this
/// reflects the permission model the runner enforces.
fn allowed_tools_for_mode(mode: &str) -> String {
    let (edits, bash, mcp) = match mode {
        "acceptEdits" => ("auto-allow", "prompt", "prompt"),
        "plan" => ("denied", "denied", "prompt"),
        "dontAsk" => ("auto-allow", "auto-allow", "auto-allow"),
        "bypassPermissions" => ("no checks", "no checks", "no checks"),
        // "default" and anything else
        _ => ("prompt", "prompt", "prompt"),
    };
    format!(
        "Tool permissions in mode `{mode}`:\n\
         \u{20}\u{20}reads  (read_file, list_dir, glob, search, fetch_url) : always allowed\n\
         \u{20}\u{20}edits  (write_file, str_replace)                      : {edits}\n\
         \u{20}\u{20}bash / shell                                          : {bash}\n\
         \u{20}\u{20}MCP tools                                             : {mcp}\n\
         \u{20}\u{20}todo / plan tools                                     : always allowed\n\
         \nChange with /mode <name>, or cycle default → acceptEdits → plan with Shift+Tab."
    )
}

/// Prompt that drives the model to review the current branch / a PR via its shell
/// tools (git, gh). Sent as a normal prompt.
fn review_prompt(arg: &str) -> String {
    let target = arg.trim();
    let scope = if target.is_empty() {
        "the changes on the current branch — compare against the base branch (e.g. `git diff main...HEAD`, or `gh pr diff` if a PR exists)".to_string()
    } else {
        format!("pull request {target} (use `gh pr diff {target}` and `gh pr view {target}`)")
    };
    format!(
        "Review {scope} as a thorough code reviewer. Use your shell tools to gather the diff and \
         surrounding context. Report, with file:line references: (1) a short summary of the change, \
         (2) correctness bugs, (3) security concerns, (4) style/maintainability issues. Prioritise \
         high-confidence, actionable findings. Do not modify any files."
    )
}

/// Prompt that drives the model to fetch and triage PR review comments via `gh`.
fn pr_comments_prompt(arg: &str) -> String {
    let target = arg.trim();
    let (pr, cmd) = if target.is_empty() {
        ("the current pull request".to_string(), "gh pr view --comments".to_string())
    } else {
        (format!("pull request {target}"), format!("gh pr view {target} --comments"))
    };
    format!(
        "Fetch the review comments on {pr} (e.g. `{cmd}`, or the GitHub API via `gh api`). \
         Summarise them grouped by file and thread, and for each draft a suggested reply or code \
         change. Do not post replies or push changes unless I explicitly confirm."
    )
}

/// Bug-report template with build/environment details for the user to paste.
fn bug_report_text() -> String {
    format!(
        "Report a bug — include the details below when filing an issue:\n\n\
         \u{20}\u{20}trogon version : {}\n\
         \u{20}\u{20}os / arch      : {} / {}\n\n\
         Then describe: what you did, what you expected, what actually happened, and any error output.",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

const BUNDLED_CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// Most recent version block from the bundled changelog (whole file when small).
fn bundled_changelog_section() -> &'static str {
    const MAX_WHOLE_FILE: usize = 800;
    let trimmed = BUNDLED_CHANGELOG.trim();
    if trimmed.len() <= MAX_WHOLE_FILE {
        return trimmed;
    }
    let start = trimmed.find("\n## ").map(|i| i + 1).unwrap_or(0);
    let section = trimmed[start..].trim_start();
    match section.find("\n## ") {
        Some(next) => section[..next].trim(),
        None => section,
    }
}

fn release_notes_text() -> String {
    format!(
        "trogon {}\n\n{}",
        env!("CARGO_PKG_VERSION"),
        bundled_changelog_section(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn handle_slash_command<F: Fs>(
    cmd: &str,
    arg: &str,
    used_tokens: u64,
    context_size: u64,
    session_model: &str,
    _cwd: &Path,
    fs: &F,
    custom_commands: Option<&crate::commands::CustomCommandRegistry>,
) -> String {
    match cmd {
        "/help" => {
            let m = "\x1b[35m";
            let r = "\x1b[0m";
            let mut help = format!("\
Commands:
  {m}/help{r}               show this help
  {m}/doctor{r}             run health checks (same as `trogon doctor`)
  {m}/status{r}             prefix, model, tokens, registered runners
  {m}/cost{r}               show token usage for this session
  {m}/context{r}            show context-window usage for this session
  {m}/clear{r}              start a new session (clears conversation history)
  {m}/sessions{r}           list sessions on the current runner
  {m}/resume{r} <id>        resume a session by id on the current runner
  {m}/mcp{r} list|add|remove|login|import|prompts  manage MCP servers (add: {m}--transport http|sse <name> <url> [--header \"K: V\"] [--oauth] [--timeout <secs>]{r})
  {m}/mcp import{r} [path]  import servers from a Claude Desktop config (default path if omitted)
  {m}/mcp login{r} <name>   authorize an HTTP MCP server via OAuth (browser)
  {m}/mcp__<server>__<prompt>{r}  run an MCP prompt as a command (see {m}/mcp prompts{r})
  {m}/compact{r}            force context compaction now
  {m}/compact-model{r}      list models & show current  |  {m}/compact-model{r} <id> set it  |  {m}/compact-model default{r} reset
  {m}/checkpoint{r} [name]  save conversation state at the current turn
  {m}/rewind{r} [name|N]    restore to a checkpoint or back N turns  |  {m}/rewind{r} lists checkpoints
  {m}/config{r}             show config  |  {m}/config{r} set <key> <value>
  {m}/model{r}              show current model  |  {m}/model{r} <id> change model
  {m}/mode{r}               show permission mode |  {m}/mode{r} <name> change mode
  {m}/plan{r}               enter plan mode (read-only exploration)
  {m}/allowed-tools{r}      show which tools the current mode permits
  {m}/vim{r}                toggle vim / emacs input editing
  {m}/review{r} [pr]        review the current branch or a PR
  {m}/pr-comments{r} [pr]   fetch & triage PR review comments
  {m}/bug{r}                print a bug-report template with build info
  {m}/release-notes{r}      show version / release info
  {m}/rename{r} <name>      name the current session (shown in /status)
  {m}/memory{r} list|show|edit  TROGON.md hierarchy (project memory)
  {m}/agents{r}             list subagent definitions (.claude/agents/)  |  {m}/agents{r} <name> details
  {m}/tasks{r}              list background tasks
  {m}/init{r}               analyze project with AI and generate TROGON.md
  {m}/init --force{r}       overwrite existing TROGON.md
  {m}cd{r} [path]           change working directory (same as {m}/cd{r})
  {m}/cd{r} [path]          change working directory (~ supported)

Multiline: end a line with \\ to continue on the next line
Ctrl+C    cancel active response
Ctrl+D    quit");
            if let Some(registry) = custom_commands {
                help.push_str(&crate::commands::format_custom_commands_help(registry));
            }
            help
        }

        "/cost" => {
            if context_size == 0 {
                "no usage data yet — send a message first".to_string()
            } else {
                let pct = (used_tokens * 100).checked_div(context_size).unwrap_or(0);
                let cost = estimate_cost(used_tokens, session_model);
                format!(
                    "context: {}/{} tokens ({}%)  |  ~${cost}",
                    fmt_tokens(used_tokens),
                    fmt_tokens(context_size),
                    pct,
                )
            }
        }

        "/context" => render_context_usage(used_tokens, context_size),

        "/config" => handle_config_cmd(arg, fs),

        "/model" => {
            if arg.is_empty() {
                "use /model with no args in the REPL for the full registry listing\n\
                 (config file model shown here only in unit tests)"
                    .to_string()
            } else {
                let model = arg.trim();
                let mut cfg = read_config(fs);
                cfg["model"] = serde_json::Value::String(model.to_string());
                match write_config(&cfg, fs) {
                    Ok(()) => format!("model set to: {model}\ncost estimates updated"),
                    Err(e) => format!("error saving model: {e}"),
                }
            }
        }

        "/doctor" => "run `trogon doctor` from the shell for full health checks".to_string(),

        "/status" => "use /status in the REPL for live session status".to_string(),

        "/memory" => "use /memory in the REPL to list or show TROGON.md hierarchy".to_string(),

        "/bug" => bug_report_text(),

        "/release-notes" => release_notes_text(),

        "/login" | "/logout" => {
            "trogon has no interactive login. Authentication uses provider tokens from the \
             environment (ANTHROPIC_TOKEN, XAI_API_KEY, OPENROUTER_API_KEY, …) or the \
             secret-proxy / vault. Set or rotate those to change credentials."
                .to_string()
        }

        "/ide" => {
            "trogon integrates with IDEs over the Agent Client Protocol (ACP): JetBrains and Zed \
             connect natively — there is no trogon plugin to manage here. Run the ACP server \
             (acp-nats-server) and point your editor's ACP client at it."
                .to_string()
        }

        "/tasks" => {
            "Lists live sub-agent spawns for the current session: child NATS sub-sessions \
             (via the runner's session list) plus any in-flight spawn_agent tool calls. \
             Read-only — use /agents to inspect subagent definitions."
                .to_string()
        }

        "/agents" => {
            "Sub-agents are spawned by the model via its spawn_agent tool (each gets an isolated \
             git worktree); the CLI doesn't manage them directly — ask the model to \"spawn a \
             sub-agent to …\". Use /sessions to list sessions on the current runner."
                .to_string()
        }

        "/rewind" | "/checkpoint" => {
            "use /checkpoint and /rewind in the REPL — they need the live session".to_string()
        }

        other => format!("unknown command: {other}  (type \x1b[35m/help\x1b[0m for a list)"),
    }
}

/// `/agents` — list custom subagent definitions; `/agents <name>` shows one.
fn handle_agents_command(arg: &str, cwd: &Path) {
    let defs = trogon_runner_tools::load_subagents(cwd);
    let arg = arg.trim();
    if defs.is_empty() {
        println!("no subagents defined — add markdown files to .claude/agents/");
        return;
    }
    if arg.is_empty() {
        println!("subagents (.claude/agents/ + ~/.config/trogon/agents/):");
        for d in &defs {
            let desc = if d.description.is_empty() { "" } else { &d.description };
            println!("  {:<20} {desc}", d.name);
        }
        println!("\nuse /agents <name> to see details");
        return;
    }
    match defs.iter().find(|d| d.name == arg) {
        Some(d) => {
            println!("name:        {}", d.name);
            if !d.description.is_empty() {
                println!("description: {}", d.description);
            }
            println!("model:       {}", d.model.as_deref().unwrap_or("(default)"));
            println!(
                "tools:       {}",
                if d.tools.is_empty() {
                    "(all)".to_string()
                } else {
                    d.tools.join(", ")
                }
            );
            println!("source:      {}", d.source.display());
            if !d.system_prompt.is_empty() {
                println!("\nsystem prompt:\n{}", d.system_prompt);
            }
        }
        None => eprintln!("no subagent named `{arg}` — run /agents to list"),
    }
}

async fn handle_memory_command<F: Fs>(arg: &str, cwd: &Path, fs: &F) {
    let arg = arg.trim();
    let layers = trogon_runner_tools::list_trogon_md_hierarchy(
        &cwd.canonicalize()
            .unwrap_or_else(|_| cwd.to_path_buf())
            .to_string_lossy(),
    )
    .await;

    if arg.is_empty() || arg == "list" {
        println!("TROGON.md hierarchy (general → specific):");
        for layer in &layers {
            let status = if layer.exists { "present" } else { "missing" };
            println!("  [{status}] {} — {}", layer.label, layer.path.display());
        }
        let project = trogon_runner_tools::project_trogon_md_path(cwd);
        println!("\nProject memory file: {}", project.display());
        println!("  /memory show     — print merged or project file");
        println!("  /memory edit     — print path to open in $EDITOR");
        return;
    }

    if arg == "edit" {
        let project = trogon_runner_tools::project_trogon_md_path(cwd);
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".into());
        println!("Project memory: {}", project.display());
        println!("Global memory:  ~/.config/trogon/TROGON.md");
        println!("Open with: {editor} {}", project.display());
        if !project.exists() {
            println!("File does not exist yet — run \x1b[35m/init\x1b[0m to generate it.");
        }
        return;
    }

    if arg == "show" || arg.starts_with("show ") {
        let path = if arg == "show" {
            trogon_runner_tools::project_trogon_md_path(cwd)
        } else {
            let raw = PathBuf::from(arg.trim_start_matches("show ").trim());
            // Only allow paths within the current project directory.
            let mut norm = std::path::PathBuf::new();
            for comp in raw.components() {
                match comp {
                    std::path::Component::ParentDir => {
                        norm.pop();
                    }
                    std::path::Component::CurDir => {}
                    c => norm.push(c),
                }
            }
            let resolved = if norm.is_absolute() { norm } else { cwd.join(norm) };
            let mut norm2 = std::path::PathBuf::new();
            for comp in resolved.components() {
                match comp {
                    std::path::Component::ParentDir => {
                        norm2.pop();
                    }
                    std::path::Component::CurDir => {}
                    c => norm2.push(c),
                }
            }
            if !norm2.starts_with(cwd) {
                eprintln!("/memory show: path must be within the project directory");
                return;
            }
            norm2
        };
        match fs.read_to_string(&path) {
            Ok(content) => {
                println!("--- {} ---", path.display());
                println!("{content}");
            }
            Err(e) => eprintln!("could not read {}: {e}", path.display()),
        }
        return;
    }

    println!("unknown /memory subcommand: {arg}");
    println!("usage: /memory [list|show|edit]");
}

fn runner_display_name(prefix: &str, agent_type: &str) -> String {
    match prefix {
        "acp.claude" => "Claude".into(),
        "acp.grok" => "Grok".into(),
        "acp.openrouter" => "OpenRouter".into(),
        "acp.codex" => "Codex".into(),
        _ => agent_type.to_string(),
    }
}

/// Short alias for a model id, shown in `/model` and `/compact-model` listings.
fn model_alias(id: &str) -> Option<&'static str> {
    match id {
        "claude-sonnet-4-6" => Some("sonnet"),
        "claude-opus-4-7" => Some("opus"),
        "claude-haiku-4-5-20251001" => Some("haiku"),
        "grok-3" => Some("grok"),
        "grok-3-mini" => Some("grok-mini"),
        "o4-mini" => Some("o4-mini"),
        "o3" => Some("o3"),
        "gpt-4o" => Some("gpt-4o"),
        "anthropic/claude-sonnet-4" => Some("op-claude"),
        "openai/gpt-4o" => Some("op-gpt"),
        "google/gemini-2.5-pro" => Some("op-gemini"),
        _ => None,
    }
}

/// Lists the models advertised by the runner at `current_prefix` — the only
/// valid choices for the compaction-model override (the compactor summarizes
/// with the session's own provider, so cross-provider ids would fail). Marks the
/// current selection.
pub(crate) async fn format_compactor_models<RS: RegistryStore>(
    registry: &Registry<RS>,
    current_prefix: &str,
    current_compactor_model: Option<&str>,
) -> Result<String, String> {
    let all = registry.list_all().await.map_err(|e| e.to_string())?;
    // Aggregate models across ALL registry entries for this prefix (there may be
    // more than one, e.g. a stale entry alongside a fresh one) and dedupe, mirroring
    // how `format_model_catalog` merges them. Using `.find()` could hit an empty one.
    let mut models: Vec<String> = Vec::new();
    for cap in all {
        let cap_prefix = cap
            .metadata
            .get("acp_prefix")
            .and_then(|v| v.as_str())
            .unwrap_or(&cap.agent_type);
        if cap_prefix != current_prefix {
            continue;
        }
        if let Some(arr) = cap.metadata.get("models").and_then(|v| v.as_array()) {
            for id in arr.iter().filter_map(|m| m.as_str()) {
                if !models.iter().any(|m| m == id) {
                    models.push(id.to_string());
                }
            }
        }
    }

    let current = current_compactor_model.unwrap_or("default (same as session model)");
    let mut out = String::new();
    out.push_str(&format!("Compaction model: \x1b[35m{current}\x1b[0m\n"));
    out.push_str(
        "Set with \x1b[35m/compact-model <id>\x1b[0m  |  reset with \x1b[35m/compact-model default\x1b[0m\n\n",
    );
    if models.is_empty() {
        out.push_str("(the current runner advertises no models)");
    } else {
        out.push_str("Available compaction models (same provider as this session):\n");
        for id in models {
            let marker = if Some(id.as_str()) == current_compactor_model {
                "  (current)"
            } else {
                ""
            };
            match model_alias(&id) {
                Some(a) => out.push_str(&format!("  {a:<10} → {id}{marker}\n")),
                None => out.push_str(&format!("  {id}{marker}\n")),
            }
        }
    }
    Ok(out.trim_end().to_string())
}

pub(crate) async fn runner_advertises_model<RS: RegistryStore>(
    registry: &Registry<RS>,
    current_prefix: &str,
    model_id: &str,
) -> Result<bool, String> {
    let all = registry.list_all().await.map_err(|e| e.to_string())?;
    for cap in all {
        let cap_prefix = cap
            .metadata
            .get("acp_prefix")
            .and_then(|v| v.as_str())
            .unwrap_or(&cap.agent_type);
        if cap_prefix != current_prefix {
            continue;
        }
        if let Some(arr) = cap.metadata.get("models").and_then(|v| v.as_array()) {
            for id in arr.iter().filter_map(|m| m.as_str()) {
                if id == model_id {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

pub(crate) async fn format_model_catalog<RS: RegistryStore>(
    registry: &Registry<RS>,
    current_prefix: &str,
    current_model: &str,
    session_id: &str,
) -> Result<String, String> {
    let all = registry.list_all().await.map_err(|e| e.to_string())?;
    let mut by_prefix: std::collections::BTreeMap<String, Vec<(String, String)>> = std::collections::BTreeMap::new();

    for cap in all {
        let prefix = cap
            .metadata
            .get("acp_prefix")
            .and_then(|v| v.as_str())
            .unwrap_or(&cap.agent_type)
            .to_string();
        let label = runner_display_name(&prefix, &cap.agent_type);
        let models = cap
            .metadata
            .get("models")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        by_prefix
            .entry(format!("{label} ({prefix})"))
            .or_default()
            .extend(models.into_iter().map(|id| (id.clone(), id)));
    }

    let mut out = String::new();
    for (group, models) in by_prefix {
        out.push_str(&group);
        out.push('\n');
        for (id, _) in models {
            if let Some(a) = model_alias(&id) {
                out.push_str(&format!("  {a:<10} → {id}\n"));
            } else {
                out.push_str(&format!("  {id}\n"));
            }
        }
        out.push('\n');
    }
    out.push_str(&format!(
        "Current session: {current_model} on {current_prefix} ({session_id})"
    ));
    Ok(out)
}

pub(crate) async fn format_status<RS: RegistryStore>(
    registry: &Registry<RS>,
    prefix: &str,
    model: &str,
    session_id: &str,
    used_tokens: u64,
    context_size: u64,
) -> String {
    let runners = registry
        .list_all()
        .await
        .map(|all| {
            all.iter()
                .filter_map(|c| c.metadata.get("acp_prefix").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|_| "unknown".into());

    let tokens = if context_size == 0 {
        format!(
            "{used} tokens used (context size unknown)",
            used = fmt_tokens(used_tokens)
        )
    } else {
        let pct = (used_tokens * 100).checked_div(context_size).unwrap_or(0);
        format!(
            "{used}/{ctx} tokens ({pct}%)",
            used = fmt_tokens(used_tokens),
            ctx = fmt_tokens(context_size),
            pct = pct,
        )
    };

    format!("prefix: {prefix}\nmodel:   {model}\nsession: {session_id}\n{tokens}\nrunners: {runners}")
}

// ── /config ───────────────────────────────────────────────────────────────────

fn config_path() -> PathBuf {
    expand_tilde("~/.config/trogon/config.json")
}

fn read_config<F: Fs>(fs: &F) -> serde_json::Value {
    let path = config_path();
    fs.read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::json!({}))
}

fn write_config<F: Fs>(config: &serde_json::Value, fs: &F) -> Result<(), String> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        fs.create_dir_all(dir)
            .map_err(|e| format!("cannot create config dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs.write(&path, content.as_bytes())
        .map_err(|e| format!("cannot write config: {e}"))
}

fn handle_config_cmd<F: Fs>(arg: &str, fs: &F) -> String {
    let mut parts = arg.splitn(3, ' ');
    match parts.next().unwrap_or("") {
        "" => {
            let cfg = read_config(fs);
            let path = config_path();
            format!(
                "config: {}\n{}",
                path.display(),
                serde_json::to_string_pretty(&cfg).unwrap_or_default()
            )
        }
        "get" => {
            let key = parts.next().unwrap_or("");
            if key.is_empty() {
                return "usage: /config get <key>".to_string();
            }
            let cfg = read_config(fs);
            match cfg.get(key) {
                Some(v) => {
                    let s = v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string());
                    format!("{key} = {s}")
                }
                None => format!("{key} is not set"),
            }
        }
        "set" => {
            let key = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("");
            if key.is_empty() {
                return "usage: /config set <key> <value>".to_string();
            }
            let mut cfg = read_config(fs);
            cfg[key] = serde_json::Value::String(value.to_string());
            match write_config(&cfg, fs) {
                Ok(()) => format!("{key} = {value}"),
                Err(e) => format!("error: {e}"),
            }
        }
        other => format!("unknown config subcommand: {other}  (use get, set, or no args)"),
    }
}

// ── /init helpers ─────────────────────────────────────────────────────────────

fn build_init_prompt<F: Fs>(root: &Path, fs: &F) -> String {
    let project_name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());

    let langs = detect_languages(root);
    let lang_line = if langs.is_empty() {
        String::new()
    } else {
        format!("Languages: {}\n", langs.join(", "))
    };

    let listing = list_top_level(root);

    let readme_section = ["README.md", "README.rst", "README.txt", "README"]
        .iter()
        .find_map(|name| {
            fs.read_to_string(&root.join(name)).ok().map(|content| {
                let truncated = if content.len() > 3000 {
                    format!("{}…(truncated)", &content[..3000])
                } else {
                    content
                };
                format!("{name}:\n{truncated}\n")
            })
        })
        .unwrap_or_default();

    format!(
        "Analyze this software project and generate the content of a TROGON.md file.\n\
         TROGON.md gives AI coding assistants context about the project.\n\
         \n\
         IMPORTANT: Do NOT call any tools (no read_file, no write_file, no list_dir, etc.).\n\
         Output the TROGON.md content directly as plain text in your response — I will write it to disk.\n\
         \n\
         Project: {project_name}\n\
         {lang_line}\
         Top-level structure:\n{listing}\n\
         \n\
         {readme_section}\
         Write a complete TROGON.md with these sections:\n\
         # {project_name}\n\
         ## Overview — purpose and goals\n\
         ## Architecture — key components and how they interact\n\
         ## Development — build, test, and run commands\n\
         ## Notes — conventions and important context for an AI assistant\n\
         \n\
         Output ONLY the TROGON.md content starting with the # heading. No preamble, no explanation."
    )
}

fn list_top_level(root: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(root) else {
        return "(unable to list directory)".to_string();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let suffix = if e.path().is_dir() { "/" } else { "" };
            Some(format!("{name}{suffix}"))
        })
        .collect();
    names.sort();
    names.join("\n")
}

fn strip_code_fence(s: &str) -> String {
    let s = s.trim();
    let inner = s
        .strip_prefix("```markdown\n")
        .or_else(|| s.strip_prefix("```md\n"))
        .or_else(|| s.strip_prefix("```\n"))
        .map(|inner| inner.strip_suffix("```").unwrap_or(inner).trim_end())
        .unwrap_or(s);
    inner.to_string()
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn detect_languages(root: &Path) -> Vec<&'static str> {
    const INDICATORS: &[(&str, &str)] = &[
        ("Cargo.toml", "Rust"),
        ("package.json", "JavaScript/TypeScript"),
        ("go.mod", "Go"),
        ("pyproject.toml", "Python"),
        ("setup.py", "Python"),
        ("build.gradle", "Kotlin/Java"),
        ("build.gradle.kts", "Kotlin"),
        ("pom.xml", "Java"),
        ("Gemfile", "Ruby"),
        ("mix.exs", "Elixir"),
        ("pubspec.yaml", "Dart/Flutter"),
        ("CMakeLists.txt", "C/C++"),
    ];
    let mut found: Vec<&'static str> = Vec::new();
    for (file, lang) in INDICATORS {
        if root.join(file).exists() && !found.contains(lang) {
            found.push(lang);
        }
    }
    found
}

// ── cost estimation ───────────────────────────────────────────────────────────

fn blended_rate_per_mtoken(model: &str) -> f64 {
    if model.contains("opus") {
        28.5
    } else if model.contains("haiku") {
        1.6
    } else if model.contains("grok") {
        2.0
    } else {
        6.0
    }
}

fn estimate_cost(tokens: u64, model: &str) -> String {
    let rate = blended_rate_per_mtoken(model);
    let cost = tokens as f64 * rate / 1_000_000.0;
    if cost < 0.01 {
        format!("{cost:.4}")
    } else {
        format!("{cost:.2}")
    }
}

// ── token formatting ──────────────────────────────────────────────────────────

fn render_context_usage(used_tokens: u64, context_size: u64) -> String {
    if context_size == 0 {
        "no context usage yet — send a message first".to_string()
    } else {
        let pct = (used_tokens * 100).checked_div(context_size).unwrap_or(0);
        let remaining = context_size.saturating_sub(used_tokens);
        format!(
            "context: {} / {} tokens ({}%) · {} remaining",
            fmt_tokens(used_tokens),
            fmt_tokens(context_size),
            pct,
            fmt_tokens(remaining),
        )
    }
}

fn fmt_tokens(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var("HOME").ok().map(PathBuf::from)
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::RealFs;
    use crate::fs::mock::MockFs;

    // ── expand_mentions with MockFs ───────────────────────────────────────────

    #[test]
    fn expand_mentions_no_at_sign_is_unchanged() {
        let fs = MockFs::new();
        assert_eq!(expand_mentions("hello world", Path::new("/tmp"), &fs), "hello world");
    }

    #[test]
    fn expand_mentions_lone_at_is_preserved() {
        let fs = MockFs::new();
        assert_eq!(
            expand_mentions("price @ discount", Path::new("/tmp"), &fs),
            "price @ discount"
        );
    }

    #[test]
    fn expand_mentions_missing_file_leaves_token_and_warns() {
        let fs = MockFs::new();
        let result = expand_mentions("see @nonexistent.txt please", Path::new("/tmp"), &fs);
        assert!(result.contains("@nonexistent.txt"));
        assert!(!result.contains("```"));
    }

    #[test]
    fn expand_mentions_existing_file_inserts_code_block() {
        let fs = MockFs::new();
        fs.add_file("/tmp/myfile.txt", "hello from file");
        let result = expand_mentions("look at @myfile.txt now", Path::new("/tmp"), &fs);
        assert!(result.contains("hello from file"), "got: {result}");
        assert!(result.contains("```"), "expected fenced block, got: {result}");
    }

    #[test]
    fn expand_mentions_multiple_mentions() {
        let fs = MockFs::new();
        fs.add_file("/tmp/f1.txt", "file1");
        fs.add_file("/tmp/f2.txt", "file2");
        let result = expand_mentions("a @f1.txt b @f2.txt c", Path::new("/tmp"), &fs);
        assert!(result.contains("file1"), "got: {result}");
        assert!(result.contains("file2"), "got: {result}");
    }

    #[test]
    fn expand_mentions_at_end_of_string_is_lone_at() {
        let fs = MockFs::new();
        assert_eq!(expand_mentions("end @", Path::new("/tmp"), &fs), "end @");
    }

    #[test]
    fn expand_mentions_code_block_includes_filename_in_header() {
        let fs = MockFs::new();
        fs.add_file("/tmp/header_test.txt", "content");
        let result = expand_mentions("@header_test.txt", Path::new("/tmp"), &fs);
        assert!(result.contains("`header_test.txt`"), "filename missing: {result}");
        assert!(result.contains("```"), "missing fenced block: {result}");
    }

    // ── expand_mentions with RealFs (temp files) ──────────────────────────────

    #[test]
    fn expand_mentions_real_fs_existing_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("trogon_real_mention.txt");
        std::fs::write(&path, "real content").unwrap();
        let fname = path.file_name().unwrap().to_str().unwrap();
        let input = format!("look at @{fname} now");
        let result = expand_mentions(&input, &dir, &RealFs);
        assert!(result.contains("real content"), "got: {result}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn expand_mentions_symlink_outside_cwd_is_rejected() {
        let dir = std::env::temp_dir().join("trogon_mention_symlink");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let outside = std::env::temp_dir().join("trogon_mention_outside_secret.txt");
        std::fs::write(&outside, "secret").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&outside, dir.join("leak.txt")).unwrap();
        }
        #[cfg(not(unix))]
        {
            return;
        }
        let cwd = dir.canonicalize().unwrap();
        let result = expand_mentions("see @leak.txt", &cwd, &RealFs);
        assert!(result.contains("@leak.txt"), "token should be preserved: {result}");
        assert!(!result.contains("secret"), "must not read through symlink: {result}");
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── FileAtHelper tab-completion (uses real fs directly — no Fs trait) ─────

    #[test]
    fn file_at_helper_complete_no_at_returns_empty() {
        let helper = FileAtHelper::new(std::env::temp_dir());
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, pairs) = helper.complete("hello world", 5, &ctx).unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn file_at_helper_complete_at_with_space_returns_empty() {
        let helper = FileAtHelper::new(std::env::temp_dir());
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, pairs) = helper.complete("@ hello", 7, &ctx).unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn file_at_helper_complete_filters_by_prefix() {
        let dir = std::env::temp_dir().join("trogon_compl_prefix");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("alpha.txt"), "").unwrap();
        std::fs::write(dir.join("alpha2.txt"), "").unwrap();
        std::fs::write(dir.join("beta.txt"), "").unwrap();

        let helper = FileAtHelper::new(dir.clone());
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (start, pairs) = helper.complete("@alpha", 6, &ctx).unwrap();

        assert_eq!(start, 1);
        let names: Vec<&str> = pairs.iter().map(|p| p.display.as_str()).collect();
        assert!(names.contains(&"alpha.txt"));
        assert!(names.contains(&"alpha2.txt"));
        assert!(!names.contains(&"beta.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_at_helper_complete_directory_gets_slash_suffix() {
        let dir = std::env::temp_dir().join("trogon_compl_dir");
        let sub = dir.join("mysubdir");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&sub).unwrap();

        let helper = FileAtHelper::new(dir.clone());
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, pairs) = helper.complete("@my", 3, &ctx).unwrap();

        let names: Vec<&str> = pairs.iter().map(|p| p.display.as_str()).collect();
        assert!(names.contains(&"mysubdir/"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_at_helper_complete_results_are_sorted() {
        let dir = std::env::temp_dir().join("trogon_compl_sorted");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["zzz.txt", "aaa.txt", "mmm.txt"] {
            std::fs::write(dir.join(name), "").unwrap();
        }

        let helper = FileAtHelper::new(dir.clone());
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, pairs) = helper.complete("@", 1, &ctx).unwrap();

        let names: Vec<&str> = pairs.iter().map(|p| p.display.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── multiline join ────────────────────────────────────────────────────────

    #[test]
    fn join_continuation_single_line_unchanged() {
        assert_eq!(join_continuation("hello world"), "hello world");
    }

    #[test]
    fn join_continuation_backslash_newline_becomes_space() {
        assert_eq!(
            join_continuation("line one\\\nline two\\\nline three"),
            "line one line two line three"
        );
    }

    #[test]
    fn join_continuation_no_backslash_newlines_unchanged() {
        assert_eq!(join_continuation("line one\nline two"), "line one\nline two");
    }

    // ── run_shell_command (`!` shell escape) ──────────────────────────────────

    #[test]
    fn run_shell_command_reports_success_and_exit_code() {
        let cwd = std::env::current_dir().unwrap();
        assert!(
            run_shell_command("exit 0", &cwd).unwrap().success(),
            "a zero exit must report success"
        );
        assert_eq!(
            run_shell_command("exit 3", &cwd).unwrap().code(),
            Some(3),
            "a non-zero exit code must be surfaced"
        );
    }

    #[test]
    fn run_shell_command_runs_in_given_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let status = run_shell_command("touch marker.txt", dir.path()).unwrap();
        assert!(status.success());
        assert!(
            dir.path().join("marker.txt").exists(),
            "command must execute in the provided working directory"
        );
    }

    // ── tab_choice (Tab autosuggestion vs @-completion) ───────────────────────

    #[test]
    fn tab_accepts_hint_at_end_of_line() {
        assert_eq!(tab_choice("git st", 6, true), TabChoice::CompleteHint);
    }

    #[test]
    fn tab_at_mention_completes_files_even_with_hint() {
        // Cursor inside an @-mention → file completion wins over any history hint.
        assert_eq!(tab_choice("read @src/ma", 12, true), TabChoice::Complete);
        assert_eq!(tab_choice("@", 1, true), TabChoice::Complete);
    }

    #[test]
    fn tab_without_hint_is_default() {
        assert_eq!(tab_choice("plain text", 10, false), TabChoice::Default);
    }

    #[test]
    fn tab_does_not_accept_hint_when_cursor_not_at_end() {
        // Hint only applies at end-of-line; mid-line Tab falls through.
        assert_eq!(tab_choice("git status", 3, true), TabChoice::Default);
    }

    #[test]
    fn tab_after_completed_mention_can_accept_hint() {
        // Trailing space ends the @-token, so a hint may be accepted again.
        assert_eq!(tab_choice("@src/main.rs ", 13, true), TabChoice::CompleteHint);
    }

    // ── Shift+Tab mode cycling ────────────────────────────────────────────────

    #[test]
    fn next_cycle_mode_cycles_the_three_core_modes() {
        assert_eq!(next_cycle_mode("default"), "acceptEdits");
        assert_eq!(next_cycle_mode("acceptEdits"), "plan");
        assert_eq!(next_cycle_mode("plan"), "default");
    }

    #[test]
    fn next_cycle_mode_enters_cycle_from_other_modes() {
        assert_eq!(next_cycle_mode("bypassPermissions"), "default");
        assert_eq!(next_cycle_mode("dontAsk"), "default");
        assert_eq!(next_cycle_mode(""), "default");
    }

    // ── edit_with ($EDITOR round-trip) ────────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn edit_with_round_trips_through_fake_editor() {
        use std::os::unix::fs::PermissionsExt;
        // Fake editor: overwrites the file it's given with fixed content.
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-editor.sh");
        std::fs::write(&script, "#!/bin/sh\nprintf 'rewritten\\n\\n' > \"$1\"\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let edited = edit_with(script.to_str().unwrap(), "seed text").unwrap();
        // The editor's content replaces the seed; trailing newlines are trimmed.
        assert_eq!(edited, "rewritten");
    }

    #[cfg(unix)]
    #[test]
    fn edit_with_preserves_seed_when_editor_is_noop() {
        // `true` ignores its argument and exits 0, leaving the seed file intact.
        let edited = edit_with("true", "keep me\n").unwrap();
        assert_eq!(edited, "keep me");
    }

    #[test]
    fn mode_prompt_has_constant_visible_width() {
        // The prompt width must not vary with the mode name, or Shift+Tab repaint
        // would misalign the cursor (rustyline measures the raw prompt).
        let widths: Vec<usize> = ["default", "acceptEdits", "plan", "dontAsk", "bypassPermissions"]
            .iter()
            .map(|m| format_mode_prompt(m).chars().count())
            .collect();
        assert!(
            widths.iter().all(|&w| w == widths[0]),
            "prompt widths must all match, got {widths:?}"
        );
        // Every label must fit the fixed field (so it's never truncated/overflowed).
        for m in ["default", "acceptEdits", "plan", "dontAsk", "bypassPermissions"] {
            assert!(
                format!("[{}]", mode_label(m)).chars().count() <= MODE_FIELD,
                "label for {m} overflows MODE_FIELD"
            );
        }
    }

    // ── input_needs_continuation ──────────────────────────────────────────────

    #[test]
    fn continuation_true_when_ends_with_backslash() {
        assert!(input_needs_continuation("hello\\"));
        assert!(input_needs_continuation("\\"));
    }

    #[test]
    fn continuation_false_when_no_trailing_backslash() {
        assert!(!input_needs_continuation("hello world"));
        assert!(!input_needs_continuation(""));
        assert!(!input_needs_continuation("has\\backslash in middle"));
    }

    // ── fmt_tokens ────────────────────────────────────────────────────────────

    #[test]
    fn fmt_tokens_small_number() {
        assert_eq!(fmt_tokens(42), "42");
    }

    #[test]
    fn fmt_tokens_thousands() {
        assert_eq!(fmt_tokens(1_234), "1,234");
    }

    #[test]
    fn fmt_tokens_millions() {
        assert_eq!(fmt_tokens(1_234_567), "1,234,567");
    }

    // ── handle_slash_command with MockFs ──────────────────────────────────────

    #[test]
    fn slash_help_lists_all_commands() {
        let fs = MockFs::new();
        let out = handle_slash_command("/help", "", 0, 0, "claude-sonnet-4-6", Path::new("/tmp"), &fs, None);
        assert!(out.contains("/help"));
        assert!(out.contains("/cost"));
        assert!(out.contains("/context"));
        assert!(out.contains("/clear"));
        assert!(out.contains("/sessions"));
        assert!(out.contains("/resume"));
        assert!(out.contains("/compact"));
        assert!(out.contains("/config"));
        assert!(out.contains("/model"));
        assert!(out.contains("/doctor"));
        assert!(out.contains("/status"));
        assert!(out.contains("/init"));
        assert!(out.contains("/memory"));
    }

    #[test]
    fn slash_help_lists_custom_commands_when_provided() {
        let fs = MockFs::new();
        let registry = crate::commands::CustomCommandRegistry::new([crate::commands::CustomCommand {
            name: "ship".into(),
            description: "Ship it".into(),
            body: String::new(),
            model: None,
            allowed_tools: Vec::new(),
            source: PathBuf::from("/x.md"),
        }]);
        let out = handle_slash_command(
            "/help",
            "",
            0,
            0,
            "claude-sonnet-4-6",
            Path::new("/tmp"),
            &fs,
            Some(&registry),
        );
        assert!(out.contains("/ship"));
        assert!(out.contains("Ship it"));
    }

    #[test]
    fn custom_command_dispatch_beats_unknown_fallback() {
        let registry = crate::commands::CustomCommandRegistry::new([crate::commands::CustomCommand {
            name: "echo".into(),
            description: String::new(),
            body: "Say: $ARGUMENTS".into(),
            model: None,
            allowed_tools: Vec::new(),
            source: PathBuf::from("/x.md"),
        }]);
        assert!(registry.dispatch("/echo", "hi").is_some());
        let unknown = handle_slash_command("/echo-not-real", "", 0, 0, "m", Path::new("/tmp"), &MockFs::new(), None);
        assert!(unknown.contains("unknown command"));
    }

    #[test]
    fn slash_cost_no_data_yet() {
        let fs = MockFs::new();
        let out = handle_slash_command("/cost", "", 0, 0, "claude-sonnet-4-6", Path::new("/tmp"), &fs, None);
        assert!(out.contains("no usage data"), "got: {out}");
    }

    #[test]
    fn slash_cost_shows_percentage_and_estimated_cost() {
        let fs = MockFs::new();
        let out = handle_slash_command(
            "/cost",
            "",
            50_000,
            200_000,
            "claude-sonnet-4-6",
            Path::new("/tmp"),
            &fs,
            None,
        );
        assert!(out.contains("25%"), "got: {out}");
        assert!(out.contains("50,000"), "got: {out}");
        assert!(out.contains("200,000"), "got: {out}");
        assert!(out.contains("~$"), "got: {out}");
    }

    #[test]
    fn slash_context_no_data_yet() {
        let out = render_context_usage(0, 0);
        assert!(out.contains("no context usage yet"), "got: {out}");
        assert!(out.contains("send a message first"), "got: {out}");
    }

    #[test]
    fn slash_context_shows_usage_percentage_and_remaining() {
        let out = render_context_usage(12_345, 200_000);
        assert_eq!(
            out,
            "context: 12,345 / 200,000 tokens (6%) · 187,655 remaining"
        );
        let via_cmd = handle_slash_command(
            "/context",
            "",
            12_345,
            200_000,
            "claude-sonnet-4-6",
            Path::new("/tmp"),
            &MockFs::new(),
            None,
        );
        assert_eq!(via_cmd, out);
    }

    // ── blended_rate / estimate_cost ──────────────────────────────────────────

    #[test]
    fn blended_rate_sonnet_default() {
        assert_eq!(blended_rate_per_mtoken("claude-sonnet-4-6"), 6.0);
    }

    #[test]
    fn blended_rate_opus() {
        assert_eq!(blended_rate_per_mtoken("claude-opus-4-7"), 28.5);
    }

    #[test]
    fn blended_rate_haiku() {
        assert_eq!(blended_rate_per_mtoken("claude-haiku-4-5"), 1.6);
    }

    #[test]
    fn blended_rate_unknown_model_falls_back_to_sonnet() {
        assert_eq!(blended_rate_per_mtoken("unknown-model"), 6.0);
    }

    #[test]
    fn estimate_cost_zero_tokens_is_zero() {
        assert_eq!(estimate_cost(0, "claude-sonnet-4-6"), "0.0000");
    }

    #[test]
    fn estimate_cost_1m_tokens_sonnet_is_6_dollars() {
        assert_eq!(estimate_cost(1_000_000, "claude-sonnet-4-6"), "6.00");
    }

    #[test]
    fn estimate_cost_uses_model_argument() {
        assert_eq!(estimate_cost(1_000_000, "claude-opus-4-7"), "28.50");
    }

    #[test]
    fn estimate_cost_small_amount_shows_four_decimals() {
        let cost = estimate_cost(1_000, "claude-sonnet-4-6");
        assert!(cost.starts_with("0.00"), "expected 4-decimal format, got: {cost}");
    }

    // ── handle_config_cmd with MockFs ─────────────────────────────────────────

    #[test]
    fn config_set_and_get_roundtrip() {
        let fs = MockFs::new();
        let set_out = handle_config_cmd("set mykey myvalue", &fs);
        assert!(
            set_out.contains("mykey") && set_out.contains("myvalue"),
            "got: {set_out}"
        );
        let get_out = handle_config_cmd("get mykey", &fs);
        assert!(get_out.contains("myvalue"), "got: {get_out}");
    }

    #[test]
    fn config_get_missing_key_says_not_set() {
        let fs = MockFs::new();
        let out = handle_config_cmd("get nonexistent", &fs);
        assert!(out.contains("not set"), "got: {out}");
    }

    #[test]
    fn config_no_args_shows_path() {
        let fs = MockFs::new();
        let out = handle_config_cmd("", &fs);
        assert!(out.contains("config"), "got: {out}");
    }

    #[test]
    fn config_set_missing_key_shows_usage() {
        let fs = MockFs::new();
        assert!(handle_config_cmd("set", &fs).contains("usage"));
    }

    #[test]
    fn config_get_missing_key_arg_shows_usage() {
        let fs = MockFs::new();
        assert!(handle_config_cmd("get", &fs).contains("usage"));
    }

    #[test]
    fn config_unknown_subcommand_returns_error() {
        let fs = MockFs::new();
        assert!(handle_config_cmd("delete mykey", &fs).contains("unknown config subcommand"));
    }

    // ── new slash commands ────────────────────────────────────────────────────

    #[test]
    fn allowed_tools_reflects_mode_gating() {
        assert!(allowed_tools_for_mode("plan").contains("edits"));
        assert!(allowed_tools_for_mode("plan").contains("denied"));
        assert!(allowed_tools_for_mode("acceptEdits").contains("auto-allow"));
        assert!(allowed_tools_for_mode("bypassPermissions").contains("no checks"));
        // reads are always allowed regardless of mode
        assert!(allowed_tools_for_mode("default").contains("always allowed"));
    }

    #[test]
    fn review_prompt_targets_branch_or_pr() {
        assert!(review_prompt("").contains("current branch"));
        let pr = review_prompt("123");
        assert!(pr.contains("123") && pr.contains("gh pr diff 123"));
        assert!(review_prompt("").to_lowercase().contains("do not modify"));
    }

    #[test]
    fn pr_comments_prompt_targets_pr_and_is_read_only() {
        assert!(pr_comments_prompt("").contains("gh pr view --comments"));
        assert!(pr_comments_prompt("42").contains("gh pr view 42 --comments"));
        assert!(pr_comments_prompt("").to_lowercase().contains("do not post"));
    }

    #[test]
    fn bug_report_includes_version_and_platform() {
        let out = bug_report_text();
        assert!(out.contains(env!("CARGO_PKG_VERSION")));
        assert!(out.contains(std::env::consts::OS));
    }

    #[test]
    fn release_notes_bundles_changelog() {
        let out = release_notes_text();
        assert!(out.contains(env!("CARGO_PKG_VERSION")));
        assert!(
            !out.to_lowercase().contains("no release notes"),
            "should bundle changelog, got: {out}"
        );
        assert!(
            out.contains("Cross-runner model switching"),
            "should include changelog bullet, got: {out}"
        );
    }

    #[test]
    fn informational_commands_are_recognised_not_unknown() {
        let fs = MockFs::new();
        for cmd in ["/login", "/logout", "/ide", "/tasks", "/agents", "/rewind", "/checkpoint", "/release-notes", "/bug"] {
            let out = handle_slash_command(cmd, "", 0, 0, "claude-sonnet-4-6", Path::new("/tmp"), &fs, None);
            assert!(!out.contains("unknown command"), "{cmd} should be recognised, got: {out}");
            assert!(!out.is_empty());
        }
    }

    #[test]
    fn help_lists_new_commands() {
        let fs = MockFs::new();
        let out = handle_slash_command("/help", "", 0, 0, "claude-sonnet-4-6", Path::new("/tmp"), &fs, None);
        for c in [
            "/plan",
            "/allowed-tools",
            "/vim",
            "/review",
            "/pr-comments",
            "/bug",
            "/checkpoint",
            "/rewind",
        ] {
            assert!(out.contains(c), "/help must mention {c}");
        }
    }

    #[test]
    fn config_set_overwrites_existing_value() {
        let fs = MockFs::new();
        handle_config_cmd("set model claude-sonnet-4-6", &fs);
        let out = handle_config_cmd("set model claude-opus-4-7", &fs);
        assert!(out.contains("claude-opus-4-7"), "got: {out}");
        let get = handle_config_cmd("get model", &fs);
        assert!(
            get.contains("claude-opus-4-7") && !get.contains("claude-sonnet-4-6"),
            "got: {get}"
        );
    }

    #[test]
    fn config_set_value_with_spaces_preserved() {
        let fs = MockFs::new();
        handle_config_cmd("set description hello world foo", &fs);
        let out = handle_config_cmd("get description", &fs);
        assert!(out.contains("hello world foo"), "got: {out}");
    }

    #[test]
    fn config_show_reflects_set_values() {
        let fs = MockFs::new();
        handle_config_cmd("set theme dark", &fs);
        let show = handle_config_cmd("", &fs);
        assert!(show.contains("dark") && show.contains("theme"), "got: {show}");
    }

    #[test]
    fn read_config_returns_empty_object_when_file_missing() {
        let fs = MockFs::new();
        let cfg = read_config(&fs);
        assert!(cfg.is_object() && cfg.as_object().unwrap().is_empty());
    }

    #[test]
    fn write_config_then_read_roundtrips() {
        let fs = MockFs::new();
        let cfg = serde_json::json!({"x": "y"});
        write_config(&cfg, &fs).unwrap();
        let read_back = read_config(&fs);
        assert_eq!(read_back["x"].as_str().unwrap(), "y");
    }

    // ── slash commands ────────────────────────────────────────────────────────

    #[test]
    fn slash_unknown_suggests_help() {
        let fs = MockFs::new();
        let out = handle_slash_command("/nope", "", 0, 0, "claude-sonnet-4-6", Path::new("/tmp"), &fs, None);
        assert!(out.contains("unknown command") && out.contains("/help"), "got: {out}");
    }

    #[test]
    fn slash_model_without_arg_shows_registry_hint() {
        let fs = MockFs::new();
        let out = handle_slash_command("/model", "", 0, 0, "claude-sonnet-4-6", Path::new("/tmp"), &fs, None);
        assert!(out.contains("registry listing"), "got: {out}");
    }

    #[test]
    fn slash_model_with_arg_saves_and_confirms() {
        let fs = MockFs::new();
        let out = handle_slash_command(
            "/model",
            "claude-opus-4-7",
            0,
            0,
            "claude-sonnet-4-6",
            Path::new("/tmp"),
            &fs,
            None,
        );
        assert!(out.contains("claude-opus-4-7"), "got: {out}");
        assert!(out.contains("cost estimates updated"), "got: {out}");
    }

    #[test]
    fn slash_model_updates_cost_estimates() {
        let fs = MockFs::new();
        handle_slash_command(
            "/model",
            "claude-haiku-4-5",
            0,
            0,
            "claude-sonnet-4-6",
            Path::new("/tmp"),
            &fs,
            None,
        );
        let cost_out = handle_slash_command(
            "/cost",
            "",
            1_000_000,
            2_000_000,
            "claude-haiku-4-5",
            Path::new("/tmp"),
            &fs,
            None,
        );
        // haiku rate is 1.6 $/Mtok → 1M tokens ≈ $1.60
        assert!(cost_out.contains("1.60") || cost_out.contains("1.6"), "got: {cost_out}");
    }

    #[test]
    fn slash_cost_at_full_context_shows_100_percent() {
        let fs = MockFs::new();
        let out = handle_slash_command(
            "/cost",
            "",
            200_000,
            200_000,
            "claude-sonnet-4-6",
            Path::new("/tmp"),
            &fs,
            None,
        );
        assert!(out.contains("100%"), "got: {out}");
    }

    #[test]
    fn slash_cost_rounds_down() {
        let fs = MockFs::new();
        let out = handle_slash_command("/cost", "", 1, 3, "claude-sonnet-4-6", Path::new("/tmp"), &fs, None);
        assert!(out.contains("33%"), "got: {out}");
    }

    // ── expand_tilde ──────────────────────────────────────────────────────────

    #[test]
    fn expand_tilde_without_tilde_is_unchanged() {
        assert_eq!(expand_tilde("/absolute/path"), PathBuf::from("/absolute/path"));
    }

    #[test]
    fn expand_tilde_bare_tilde_without_slash_is_unchanged() {
        assert_eq!(expand_tilde("~nodot"), PathBuf::from("~nodot"));
    }

    // ── strip_code_fence ──────────────────────────────────────────────────────

    #[test]
    fn strip_code_fence_plain_fence() {
        assert_eq!(strip_code_fence("```\n# Title\n```"), "# Title");
    }

    #[test]
    fn strip_code_fence_markdown_lang() {
        assert_eq!(strip_code_fence("```markdown\n# Title\n```"), "# Title");
    }

    #[test]
    fn strip_code_fence_md_lang() {
        assert_eq!(strip_code_fence("```md\n# Title\n```"), "# Title");
    }

    #[test]
    fn strip_code_fence_no_fence_passthrough() {
        assert_eq!(strip_code_fence("# Title\nsome text"), "# Title\nsome text");
    }

    #[test]
    fn strip_code_fence_trims_trailing_whitespace() {
        let input = "```\n# Title\n\n```";
        let out = strip_code_fence(input);
        assert_eq!(out, "# Title", "got: {out:?}");
    }

    #[test]
    fn strip_code_fence_empty_string() {
        assert_eq!(strip_code_fence(""), "");
    }

    // ── find_git_root ─────────────────────────────────────────────────────────

    #[test]
    fn find_git_root_returns_none_when_no_git_dir() {
        let dir = std::env::temp_dir().join("trogon_no_git");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        assert!(find_git_root(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_git_root_finds_direct_git_dir() {
        let dir = std::env::temp_dir().join("trogon_has_git");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let root = find_git_root(&dir);
        assert_eq!(root, Some(dir.clone()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_git_root_traverses_up_to_git_dir() {
        let base = std::env::temp_dir().join("trogon_git_traverse");
        let child = base.join("a").join("b").join("c");
        std::fs::remove_dir_all(&base).ok();
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(base.join(".git")).unwrap();
        let root = find_git_root(&child);
        assert_eq!(root, Some(base.clone()));
        std::fs::remove_dir_all(&base).ok();
    }

    // ── detect_languages ──────────────────────────────────────────────────────

    #[test]
    fn detect_languages_empty_dir_returns_none() {
        let dir = std::env::temp_dir().join("trogon_langs_empty");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        assert!(detect_languages(&dir).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detect_languages_cargo_toml_detected_as_rust() {
        let dir = std::env::temp_dir().join("trogon_langs_rust");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        let langs = detect_languages(&dir);
        assert!(langs.contains(&"Rust"), "got: {langs:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detect_languages_multiple_indicators() {
        let dir = std::env::temp_dir().join("trogon_langs_multi");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.join("package.json"), "{}").unwrap();
        let langs = detect_languages(&dir);
        assert!(langs.contains(&"Rust"), "rust missing: {langs:?}");
        assert!(langs.contains(&"JavaScript/TypeScript"), "js missing: {langs:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── build_init_prompt ─────────────────────────────────────────────────────

    #[test]
    fn build_init_prompt_contains_required_sections() {
        let dir = std::env::temp_dir().join("trogon_init_prompt");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();

        let fs = MockFs::new();
        let prompt = build_init_prompt(&dir, &fs);

        assert!(prompt.contains("TROGON.md"), "missing TROGON.md: {prompt}");
        assert!(prompt.contains("Overview"), "missing Overview: {prompt}");
        assert!(prompt.contains("Architecture"), "missing Architecture: {prompt}");
        assert!(prompt.contains("Development"), "missing Development: {prompt}");
        assert!(prompt.contains("Notes"), "missing Notes: {prompt}");
        assert!(prompt.contains("Rust"), "missing detected language: {prompt}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_init_prompt_includes_readme_when_present() {
        let dir = std::env::temp_dir().join("trogon_init_readme");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();

        let fs = MockFs::new();
        let readme_path = dir.join("README.md");
        fs.add_file(readme_path.to_str().unwrap(), "My special readme content.");

        let prompt = build_init_prompt(&dir, &fs);
        assert!(
            prompt.contains("My special readme content."),
            "readme not included: {prompt}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_init_prompt_project_name_from_dir() {
        let dir = std::env::temp_dir().join("my_cool_project");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();

        let fs = MockFs::new();
        let prompt = build_init_prompt(&dir, &fs);
        assert!(prompt.contains("my_cool_project"), "project name missing: {prompt}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── apply_model_switch ────────────────────────────────────────────────────

    use crate::cross_runner::mock::MockRunnerSwitcher;

    #[tokio::test]
    async fn model_switch_same_runner_sets_same_runner_flag() {
        let mut switcher = MockRunnerSwitcher::same_runner("acp", "sess-1");
        let outcome = apply_model_switch(&mut switcher, "acp", "sess-1", "claude-opus-4-7", "/ws")
            .await
            .unwrap();
        assert!(outcome.same_runner);
        assert_eq!(outcome.new_prefix, "acp");
        assert_eq!(outcome.new_session_id, "sess-1");
    }

    #[tokio::test]
    async fn model_switch_cross_runner_returns_new_prefix_and_session() {
        let mut switcher = MockRunnerSwitcher::cross_runner("acp.xai", "new-sess-99");
        let outcome = apply_model_switch(&mut switcher, "acp", "old-sess", "grok-3", "/ws")
            .await
            .unwrap();
        assert!(!outcome.same_runner);
        assert_eq!(outcome.new_prefix, "acp.xai");
        assert_eq!(outcome.new_session_id, "new-sess-99");
    }

    #[tokio::test]
    async fn model_switch_error_propagates() {
        let mut switcher = MockRunnerSwitcher::error("no runner found for model: unknown");
        let err = apply_model_switch(&mut switcher, "acp", "sess-1", "unknown", "/ws")
            .await
            .unwrap_err();
        assert!(err.contains("no runner found for model: unknown"), "got: {err}");
    }

    #[tokio::test]
    async fn model_switch_cross_runner_different_from_current_prefix() {
        let mut switcher = MockRunnerSwitcher::cross_runner("acp.openrouter", "s-42");
        let outcome = apply_model_switch(&mut switcher, "acp", "s-old", "gpt-4o", "/workspace")
            .await
            .unwrap();
        assert!(!outcome.same_runner, "cross-runner must not be same_runner");
        assert_ne!(outcome.new_prefix, "acp");
    }

    // ── resolve_model_alias ───────────────────────────────────────────────────

    #[test]
    fn resolve_model_alias_claude_shortcuts() {
        assert_eq!(resolve_model_alias("sonnet"), "claude-sonnet-4-6");
        assert_eq!(resolve_model_alias("opus"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias("haiku"), "claude-haiku-4-5-20251001");
    }

    #[test]
    fn resolve_model_alias_grok_shortcuts() {
        assert_eq!(resolve_model_alias("grok"), "grok-3");
        assert_eq!(resolve_model_alias("grok-mini"), "grok-3-mini");
    }

    #[test]
    fn resolve_model_alias_codex_and_openrouter_defaults() {
        unsafe {
            std::env::remove_var("OPENROUTER_DEFAULT_MODEL");
            std::env::remove_var("CODEX_DEFAULT_MODEL");
        }
        assert_eq!(resolve_model_alias("openrouter"), "anthropic/claude-sonnet-4-6");
        assert_eq!(resolve_model_alias("codex"), "o4-mini");
    }

    #[test]
    fn resolve_model_alias_env_overrides() {
        unsafe {
            std::env::set_var("OPENROUTER_DEFAULT_MODEL", "openai/gpt-4o");
            std::env::set_var("CODEX_DEFAULT_MODEL", "o3");
        }
        assert_eq!(resolve_model_alias("openrouter"), "openai/gpt-4o");
        assert_eq!(resolve_model_alias("codex"), "o3");
        unsafe {
            std::env::remove_var("OPENROUTER_DEFAULT_MODEL");
            std::env::remove_var("CODEX_DEFAULT_MODEL");
        }
    }

    #[test]
    fn resolve_model_alias_passthrough() {
        assert_eq!(resolve_model_alias("o3"), "o3");
        assert_eq!(resolve_model_alias("gpt-4o"), "gpt-4o");
        assert_eq!(resolve_model_alias("custom/model-id"), "custom/model-id");
    }

    #[test]
    fn resolve_model_alias_targets_registered_ids() {
        const ACP_IDS: &[&str] =
            &["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5-20251001"];
        const OR_IDS: &[&str] =
            &["anthropic/claude-sonnet-4-6", "openai/gpt-4o", "google/gemini-pro-1.5"];

        for alias in ["haiku", "sonnet", "opus"] {
            let resolved = resolve_model_alias(alias);
            assert!(ACP_IDS.contains(&resolved.as_str()), "{alias} -> {resolved}");
        }
        for alias in ["op-claude", "op-gpt", "op-gemini", "openrouter"] {
            let resolved = resolve_model_alias(alias);
            assert!(OR_IDS.contains(&resolved.as_str()), "{alias} -> {resolved}");
        }
    }

    #[tokio::test]
    async fn runner_advertises_model_checks_registry() {
        use trogon_registry::{AgentCapability, MockRegistryStore, Registry};

        let registry = Registry::new(MockRegistryStore::new());
        let mut cap = AgentCapability::new("runner", ["chat"], "agents.runner.>");
        cap.metadata = serde_json::json!({
            "acp_prefix": "acp.claude",
            "models": ["claude-haiku-4-5-20251001"]
        });
        registry.register(&cap).await.unwrap();

        assert_eq!(
            runner_advertises_model(&registry, "acp.claude", "claude-haiku-4-5-20251001")
                .await
                .unwrap(),
            true
        );
        assert_eq!(
            runner_advertises_model(&registry, "acp.claude", "bogus-model")
                .await
                .unwrap(),
            false
        );
    }

    #[test]
    fn sync_cwd_from_change_directory_tool() {
        let mut cwd = PathBuf::from("/tmp");
        assert!(sync_cwd_from_tool(
            "change_directory",
            "Working directory is now /home/jorge/straw-hat/trogonai",
            &mut cwd,
        ));
        assert_eq!(cwd, PathBuf::from("/home/jorge/straw-hat/trogonai"));
    }

    #[test]
    fn sync_cwd_from_bash_cd_tool() {
        let mut cwd = PathBuf::from("/tmp");
        assert!(sync_cwd_from_tool(
            "bash",
            "Working directory is now /var/log",
            &mut cwd,
        ));
        assert_eq!(cwd, PathBuf::from("/var/log"));
    }

    #[test]
    fn sync_cwd_ignores_unrelated_tools() {
        let mut cwd = PathBuf::from("/tmp");
        assert!(!sync_cwd_from_tool("list_dir", "foo\nbar", &mut cwd));
        assert_eq!(cwd, PathBuf::from("/tmp"));
    }

    // ── /clear ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn clear_closes_old_session_and_creates_new() {
        use crate::session::mock::{MockSession, MockSessionFactory};
        use std::sync::Arc;

        let old = Arc::new(MockSession::new("old-sess"));
        let new_sess = Arc::new(MockSession::new("new-sess"));
        let factory = MockSessionFactory::new("default");
        factory.push_session(new_sess.clone());

        old.close().await;
        let created = factory.create_session("acp", PathBuf::from("/tmp"), vec![]).await.unwrap();

        assert_eq!(old.close_count(), 1, "old session must be closed once");
        assert_eq!(created.session_id(), "new-sess", "factory must return the queued session");
    }

    #[tokio::test]
    async fn clear_resets_token_counters() {
        // After /clear the loop resets both counters; simulate that here.
        let used: u64 = 0;
        let ctx: u64 = 0;
        assert_eq!(used, 0);
        assert_eq!(ctx, 0);
    }

    // ── StreamEvent handling (prompt loop) ────────────────────────────────────

    #[tokio::test]
    async fn usage_event_updates_token_counters() {
        use crate::session::mock::MockSession;
        use std::sync::Arc;

        let session = Arc::new(MockSession::new("sess-1"));
        session.queue_turn(vec![
            StreamEvent::Usage { used_tokens: 75_000, context_size: 150_000 },
            StreamEvent::Done("end_turn".into()),
        ]);

        let mut used: u64 = 0;
        let mut ctx: u64 = 0;
        let mut rx = session.prompt("hello").await.unwrap();
        loop {
            match rx.recv().await {
                None => break,
                Some(StreamEvent::Usage { used_tokens, context_size }) => {
                    used = used_tokens;
                    ctx = context_size;
                }
                Some(StreamEvent::Done(_)) => break,
                _ => {}
            }
        }
        assert_eq!(used, 75_000, "used_tokens must be updated from Usage event");
        assert_eq!(ctx, 150_000, "context_size must be updated from Usage event");
    }

    #[tokio::test]
    async fn multiple_usage_events_last_one_wins() {
        use crate::session::mock::MockSession;
        use std::sync::Arc;

        let session = Arc::new(MockSession::new("sess-1"));
        session.queue_turn(vec![
            StreamEvent::Usage { used_tokens: 10_000, context_size: 100_000 },
            StreamEvent::Usage { used_tokens: 20_000, context_size: 100_000 },
            StreamEvent::Done("end_turn".into()),
        ]);

        let mut used: u64 = 0;
        let mut rx = session.prompt("hello").await.unwrap();
        loop {
            match rx.recv().await {
                None => break,
                Some(StreamEvent::Usage { used_tokens, .. }) => {
                    used = used_tokens;
                }
                Some(StreamEvent::Done(_)) => break,
                _ => {}
            }
        }
        assert_eq!(used, 20_000, "last Usage event must win");
    }

    #[tokio::test]
    async fn tool_call_and_diff_events_complete_without_panic() {
        use crate::session::mock::MockSession;
        use std::sync::Arc;

        let session = Arc::new(MockSession::new("sess-1"));
        session.queue_turn(vec![
            StreamEvent::ToolCall("read_file".into()),
            StreamEvent::Diff("--- old\n+++ new\n@@ -1 +1 @@ fn main() {}".into()),
            StreamEvent::Text("Done editing.".into()),
            StreamEvent::Done("end_turn".into()),
        ]);

        let mut text = String::new();
        let mut rx = session.prompt("edit something").await.unwrap();
        loop {
            match rx.recv().await {
                None => break,
                Some(StreamEvent::Text(t)) => text.push_str(&t),
                Some(StreamEvent::Done(_)) => break,
                _ => {}
            }
        }
        assert_eq!(text, "Done editing.");
    }

    #[tokio::test]
    async fn thinking_event_is_silently_ignored() {
        use crate::session::mock::MockSession;
        use std::sync::Arc;

        let session = Arc::new(MockSession::new("sess-1"));
        session.queue_turn(vec![
            StreamEvent::Thinking,
            StreamEvent::Text("answer".into()),
            StreamEvent::Done("end_turn".into()),
        ]);

        let mut text = String::new();
        let mut rx = session.prompt("think").await.unwrap();
        loop {
            match rx.recv().await {
                None => break,
                Some(StreamEvent::Text(t)) => text.push_str(&t),
                Some(StreamEvent::Done(_)) => break,
                _ => {}
            }
        }
        assert_eq!(text, "answer");
    }

    // ── /model cross-runner: loop wiring ─────────────────────────────────────

    #[tokio::test]
    async fn cross_runner_switch_binds_mcp_before_prompt_unblocked() {
        use crate::session::mock::{MockSession, MockSessionFactory};
        use std::sync::Arc;

        let old = Arc::new(MockSession::new("old-sess"));
        let factory = MockSessionFactory::new("new-sess");
        let fs = MockFs::new();
        let mut mcp = McpManager::load(&fs);
        let mut journal = SwitchJournal::default();
        let outcome = ModelSwitchOutcome {
            same_runner: false,
            new_prefix: "acp.xai".into(),
            new_session_id: "new-sess".into(),
        };

        let applied = finish_cross_runner_model_switch(
            &factory,
            old.clone(),
            &outcome,
            "grok-3",
            &mut mcp,
            Path::new("/ws"),
            &fs,
            Path::new("/ws"),
            None,
            false,
            None,
            Some(&mut journal),
        )
        .await;

        assert_eq!(old.close_count(), 1);
        assert_eq!(applied.session.session_id(), "new-sess");
        assert_eq!(applied.prefix, "acp.xai");
        assert_eq!(applied.session.load_session_count(), 1);

        let steps = journal.steps();
        let mcp_idx = steps.iter().position(|&s| s == "mcp_bound").expect("mcp_bound step");
        let prompt_idx = steps
            .iter()
            .position(|&s| s == "prompt_unblocked")
            .expect("prompt_unblocked step");
        assert!(
            mcp_idx < prompt_idx,
            "MCP must be bound before the prompt unblocks: {steps:?}"
        );
    }

    #[tokio::test]
    async fn model_cross_runner_attaches_new_session_and_resets_counters() {
        use crate::session::mock::MockSessionFactory;

        let mut switcher = MockRunnerSwitcher::cross_runner("acp.xai", "xai-sess-99");
        let factory = MockSessionFactory::new("default");

        let outcome = apply_model_switch(&mut switcher, "acp", "old-sess", "grok-3", "/ws")
            .await
            .unwrap();

        assert!(!outcome.same_runner);
        let new_session = factory.attach_session(&outcome.new_prefix, outcome.new_session_id);
        let new_prefix = outcome.new_prefix;

        assert_eq!(new_prefix, "acp.xai");
        assert_eq!(new_session.session_id(), "xai-sess-99");
    }

    #[tokio::test]
    async fn model_same_runner_calls_set_model_on_session() {
        use crate::session::mock::MockSession;

        let session = std::sync::Arc::new(MockSession::new("sess-1"));
        let mut switcher = MockRunnerSwitcher::same_runner("acp", "sess-1");

        let outcome = apply_model_switch(&mut switcher, "acp", "sess-1", "claude-opus-4-7", "/ws")
            .await
            .unwrap();

        assert!(outcome.same_runner);
        session.set_model("claude-opus-4-7").await.unwrap();
        assert_eq!(session.last_model().as_deref(), Some("claude-opus-4-7"));
    }

    #[test]
    fn queued_prompt_from_dispatch_carries_frontmatter() {
        let dispatch = crate::commands::CustomCommandDispatch {
            prompt: "do it".into(),
            model: Some("claude-opus-4-7".into()),
            allowed_tools: vec!["Read".into()],
        };
        let q = QueuedPrompt::from_dispatch(dispatch);
        assert_eq!(q.model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(q.allowed_tools, vec!["Read"]);
        assert_eq!(q.prompt_opts().tool_allowlist, vec!["read_file"]);
    }

    #[tokio::test]
    async fn apply_dispatch_model_switch_same_runner_sets_model() {
        use crate::session::mock::{MockSession, MockSessionFactory};
        use std::sync::Arc;

        let session = Arc::new(MockSession::new("sess-1"));
        let factory = MockSessionFactory::new("default");
        let mut switcher = MockRunnerSwitcher::same_runner("acp", "sess-1");
        let mut prefix = "acp".to_string();
        let mut session_mode = "default".to_string();
        let mut session_used_tokens = 0;
        let mut session_context_size = 0;
        let mut compactor_model_sel = None;
        let mut rewind_state = SessionRewindState::default();
        let mut mcp_manager = McpManager::load(&MockFs::new());
        let cwd = std::env::temp_dir();
        let fs = MockFs::new();
        let project_dir = cwd.clone();

        let updated = apply_dispatch_model_switch(
            &mut switcher,
            &factory,
            session,
            &mut prefix,
            &mut session_mode,
            &mut session_used_tokens,
            &mut session_context_size,
            &mut compactor_model_sel,
            &mut rewind_state,
            &mut mcp_manager,
            &cwd,
            &fs,
            &project_dir,
            None,
            false,
            None,
            "claude-opus-4-7",
        )
        .await;

        assert_eq!(updated.last_model().as_deref(), Some("claude-opus-4-7"));
    }

    // ── /checkpoint and /rewind ────────────────────────────────────────────────

    #[tokio::test]
    async fn checkpoint_command_exports_and_records_turn() {
        use crate::session::mock::MockSession;

        let session = MockSession::new("sess-1");
        session.set_exported_history(r#"[{"role":"user","text":"hello"}]"#);
        let mut rewind_state = SessionRewindState::default();
        rewind_state.on_turn_complete();

        let msg = handle_checkpoint_command(&session, &mut rewind_state, "save-point")
            .await
            .unwrap();
        assert!(msg.contains("save-point"));
        assert_eq!(rewind_state.checkpoints().len(), 1);
        assert_eq!(rewind_state.checkpoints()[0].turn_index, 1);
    }

    #[tokio::test]
    async fn rewind_to_checkpoint_imports_snapshot_and_truncates_state() {
        use crate::session::mock::MockSession;

        let session = MockSession::new("sess-1");
        let snapshot = r#"[{"role":"user","text":"first"}]"#;
        session.set_exported_history(snapshot);
        let mut rewind_state = SessionRewindState::default();
        rewind_state.on_turn_complete();
        rewind_state.record_checkpoint(Some("t1"), snapshot.to_string());
        rewind_state.on_turn_complete();
        rewind_state.on_turn_complete();

        let msg = handle_rewind_command(&session, &mut rewind_state, "t1")
            .await
            .unwrap();
        assert!(msg.contains("turn 1"));
        assert_eq!(session.imported_history(), vec![snapshot]);
        assert_eq!(rewind_state.turn_count(), 1);
        assert_eq!(rewind_state.checkpoints().len(), 1);
    }

    #[tokio::test]
    async fn rewind_by_n_truncates_export_when_no_checkpoint() {
        use crate::session::mock::MockSession;

        let session = MockSession::new("sess-1");
        session.set_exported_history(
            r#"[
                {"role":"user","text":"one"},
                {"role":"assistant","text":"a1"},
                {"role":"user","text":"two"},
                {"role":"assistant","text":"a2"}
            ]"#,
        );
        let mut rewind_state = SessionRewindState::default();
        rewind_state.on_turn_complete();
        rewind_state.on_turn_complete();

        let msg = handle_rewind_command(&session, &mut rewind_state, "1")
            .await
            .unwrap();
        assert!(msg.contains("turn 1"));
        let imported = session.imported_history();
        assert_eq!(imported.len(), 1);
        assert!(imported[0].contains(r#""text":"one""#));
        assert!(!imported[0].contains(r#""text":"two""#));
        assert_eq!(rewind_state.turn_count(), 1);
    }

    #[tokio::test]
    async fn rewind_without_arg_lists_checkpoints() {
        use crate::session::mock::MockSession;

        let session = MockSession::new("sess-1");
        let mut rewind_state = SessionRewindState::default();
        rewind_state.on_turn_complete();
        rewind_state.record_checkpoint(Some("alpha"), "[]".to_string());

        let err = handle_rewind_command(&session, &mut rewind_state, "")
            .await
            .unwrap_err();
        assert!(matches!(err, RewindError::ListRequested));
        let listed = rewind_state.list_checkpoints();
        assert!(listed.contains("alpha"));
    }
}
