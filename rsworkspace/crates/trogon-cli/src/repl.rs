use crate::session::{StreamEvent, TrogonSession};
use async_nats::Client;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Context, Editor, Helper};
use std::io::Write;
use std::path::{Path, PathBuf};

const HISTORY_PATH: &str = "~/.local/share/trogon/history";

// ── FileAtHelper ──────────────────────────────────────────────────────────────

/// rustyline helper: tab-completes `@<path>` tokens and enables `\`-continuation
/// for multiline input.
struct FileAtHelper {
    cwd: PathBuf,
}

impl Default for FileAtHelper {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

impl Helper for FileAtHelper {}

impl Completer for FileAtHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let before = &line[..pos];
        let Some(at_pos) = before.rfind('@') else {
            return Ok((pos, vec![]));
        };
        let partial = &before[at_pos + 1..];
        if partial.contains(' ') {
            return Ok((pos, vec![]));
        }
        let (dir, prefix) = if let Some(slash) = partial.rfind('/') {
            (self.cwd.join(&partial[..=slash]), &partial[slash + 1..])
        } else {
            (self.cwd.clone(), partial)
        };
        let mut pairs: Vec<Pair> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(prefix) {
                    let suffix = if entry.path().is_dir() { "/" } else { "" };
                    let replacement = format!("{name}{suffix}");
                    pairs.push(Pair { display: replacement.clone(), replacement });
                }
            }
        }
        pairs.sort_by(|a, b| a.display.cmp(&b.display));
        Ok((at_pos + 1, pairs))
    }
}

impl Hinter for FileAtHelper {
    type Hint = String;
    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<String> {
        None
    }
}

impl Highlighter for FileAtHelper {}

/// A line ending with `\` continues on the next line.
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

/// Scans `text` for `@<path>` tokens and replaces each with a fenced code block
/// containing the file's contents, resolved relative to `cwd`.
/// Unresolvable paths are left as-is with a warning on stderr.
fn expand_mentions(text: &str, cwd: &Path) -> String {
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
                match std::fs::read_to_string(cwd.join(path_str)) {
                    Ok(content) => {
                        result.push_str(&format!("`{path_str}`:\n```\n{content}\n```"));
                    }
                    Err(_) => {
                        eprintln!("warning: @{path_str}: file not found or not readable");
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

/// Join continuation lines: `\` at the end of a line is replaced by a space.
fn join_continuation(s: &str) -> String {
    s.replace("\\\n", " ")
}

// ── REPL entry point ──────────────────────────────────────────────────────────

pub async fn run(nats: Client, prefix: &str, cwd: PathBuf) -> anyhow::Result<()> {
    let prefix = prefix.to_string();
    let mut session = TrogonSession::new(nats.clone(), &prefix, cwd.clone()).await?;

    let history_path = expand_tilde(HISTORY_PATH);
    if let Some(dir) = history_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    let mut rl: Editor<FileAtHelper, _> = Editor::new()?;
    rl.set_helper(Some(FileAtHelper { cwd: cwd.clone() }));
    let _ = rl.load_history(&history_path);

    eprintln!("trogon — session {} (Ctrl+D to quit)", session.session_id);

    // Accumulated context-window usage across the session.
    let mut session_used_tokens: u64 = 0;
    let mut session_context_size: u64 = 0;

    loop {
        match rl.readline("> ") {
            Ok(raw_line) => {
                // Join backslash-continuation lines, then expand @mentions.
                let line = join_continuation(&raw_line).trim().to_string();
                if line.is_empty() {
                    continue;
                }

                if line.starts_with('/') {
                    let mut parts = line.splitn(2, ' ');
                    let cmd = parts.next().unwrap_or("");
                    let arg = parts.next().unwrap_or("");
                    if cmd == "/clear" {
                        match TrogonSession::new(nats.clone(), &prefix, cwd.clone()).await {
                            Ok(s) => {
                                session = s;
                                session_used_tokens = 0;
                                session_context_size = 0;
                                eprintln!("session cleared — new session {}", session.session_id);
                            }
                            Err(e) => eprintln!("error: {e}"),
                        }
                    } else {
                        println!("{}", handle_slash_command(cmd, arg, session_used_tokens, session_context_size));
                    }
                    continue;
                }

                let _ = rl.add_history_entry(&raw_line);

                let expanded = expand_mentions(&line, &cwd);
                match session.prompt(&expanded).await {
                    Err(e) => eprintln!("error: {e}"),
                    Ok(mut rx) => {
                        let mut stdout = std::io::stdout();
                        let ctrl_c = tokio::signal::ctrl_c();
                        tokio::pin!(ctrl_c);

                        loop {
                            tokio::select! {
                                biased;
                                _ = &mut ctrl_c => {
                                    session.cancel().await;
                                    eprintln!("\n[cancelled]");
                                    break;
                                }
                                event = rx.recv() => {
                                    match event {
                                        None => break,
                                        Some(StreamEvent::Text(text)) => {
                                            print!("{text}");
                                            let _ = stdout.flush();
                                        }
                                        Some(StreamEvent::Thinking) => {}
                                        Some(StreamEvent::ToolCall(name)) => {
                                            eprintln!("\n[tool: {name}]");
                                        }
                                        Some(StreamEvent::Diff(diff)) => {
                                            eprintln!("{diff}");
                                        }
                                        Some(StreamEvent::Usage { used_tokens, context_size }) => {
                                            session_used_tokens = used_tokens;
                                            session_context_size = context_size;
                                        }
                                        Some(StreamEvent::Done(reason)) => {
                                            if reason == "cancelled" {
                                                eprintln!("\n[cancelled]");
                                            } else {
                                                println!();
                                                if session_context_size > 0 {
                                                    let pct = session_used_tokens * 100 / session_context_size;
                                                    eprintln!(
                                                        "\x1b[2m[context: {}/{} tokens ({}%)]\x1b[0m",
                                                        fmt_tokens(session_used_tokens),
                                                        fmt_tokens(session_context_size),
                                                        pct,
                                                    );
                                                }
                                            }
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C during the readline prompt (not during streaming).
                session.cancel().await;
                eprintln!("(Ctrl+C)");
            }
            Err(ReadlineError::Eof) => {
                eprintln!("bye");
                break;
            }
            Err(e) => {
                eprintln!("readline error: {e}");
                break;
            }
        }
    }

    let _ = rl.save_history(&history_path);
    Ok(())
}

fn handle_slash_command(cmd: &str, arg: &str, used_tokens: u64, context_size: u64) -> String {
    match cmd {
        "/help" => "\
Commands:
  /help               show this help
  /cost               show token usage for this session
  /clear              start a new session (clears conversation history)
  /compact            force context compaction
  /config             show config  |  /config set <key> <value>
  /model <id>         change model for this session (not yet implemented)
  /init               generate TROGON.md for this project (not yet implemented)

Multiline: end a line with \\ to continue on the next line
Ctrl+C    cancel active response
Ctrl+D    quit"
            .to_string(),

        "/cost" => {
            if context_size == 0 {
                "no usage data yet — send a message first".to_string()
            } else {
                let pct = used_tokens * 100 / context_size;
                format!(
                    "context: {}/{} tokens ({}%)",
                    fmt_tokens(used_tokens),
                    fmt_tokens(context_size),
                    pct,
                )
            }
        }

        "/compact" => {
            "compaction is triggered automatically at 85% context usage.\n\
             Force compact is not yet implemented."
                .to_string()
        }

        "/config" => handle_config_cmd(arg),

        "/model" => {
            if arg.is_empty() {
                "usage: /model <model-id>  (not yet implemented — requires runner support)".to_string()
            } else {
                format!("/model not yet implemented — requires runner support")
            }
        }

        "/init" => "not yet implemented".to_string(),

        other => format!("unknown command: {other}  (type /help for a list)"),
    }
}

// ── /config ───────────────────────────────────────────────────────────────────

fn config_path() -> PathBuf {
    expand_tilde("~/.config/trogon/config.json")
}

fn read_config() -> serde_json::Value {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::json!({}))
}

fn write_config(config: &serde_json::Value) -> Result<(), String> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create config dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(&path, content).map_err(|e| format!("cannot write config: {e}"))
}

fn handle_config_cmd(arg: &str) -> String {
    let mut parts = arg.splitn(3, ' ');
    match parts.next().unwrap_or("") {
        "" => {
            let cfg = read_config();
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
            let cfg = read_config();
            match cfg.get(key) {
                Some(v) => format!("{key} = {v}"),
                None => format!("{key} is not set"),
            }
        }
        "set" => {
            let key = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("");
            if key.is_empty() {
                return "usage: /config set <key> <value>".to_string();
            }
            let mut cfg = read_config();
            cfg[key] = serde_json::Value::String(value.to_string());
            match write_config(&cfg) {
                Ok(()) => format!("{key} = {value}"),
                Err(e) => format!("error: {e}"),
            }
        }
        other => format!("unknown config subcommand: {other}  (use get, set, or no args)"),
    }
}

// ── token formatting ──────────────────────────────────────────────────────────

/// Format a token count with thousands separator (e.g. 12345 → "12,345").
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
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var("HOME").ok().map(PathBuf::from) {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── expand_mentions ───────────────────────────────────────────────────────

    #[test]
    fn expand_mentions_no_at_sign_is_unchanged() {
        let result = expand_mentions("hello world", Path::new("/tmp"));
        assert_eq!(result, "hello world");
    }

    #[test]
    fn expand_mentions_lone_at_is_preserved() {
        let result = expand_mentions("price @ discount", Path::new("/tmp"));
        assert_eq!(result, "price @ discount");
    }

    #[test]
    fn expand_mentions_missing_file_leaves_token_and_warns() {
        let result = expand_mentions("see @nonexistent.txt please", Path::new("/tmp"));
        assert!(result.contains("@nonexistent.txt"));
        assert!(!result.contains("```"));
    }

    #[test]
    fn expand_mentions_existing_file_inserts_code_block() {
        let dir = std::env::temp_dir();
        let file_path = dir.join("trogon_test_mention.txt");
        {
            let mut f = std::fs::File::create(&file_path).unwrap();
            f.write_all(b"hello from file").unwrap();
        }
        let filename = file_path.file_name().unwrap().to_str().unwrap();
        let input = format!("look at @{filename} now");
        let result = expand_mentions(&input, &dir);
        assert!(result.contains("hello from file"), "got: {result}");
        assert!(result.contains("```"), "expected fenced block, got: {result}");
        let _ = std::fs::remove_file(&file_path);
    }

    #[test]
    fn expand_mentions_multiple_mentions() {
        let dir = std::env::temp_dir();
        let f1 = dir.join("trogon_m1.txt");
        let f2 = dir.join("trogon_m2.txt");
        std::fs::write(&f1, "file1").unwrap();
        std::fs::write(&f2, "file2").unwrap();
        let input = "a @trogon_m1.txt b @trogon_m2.txt c";
        let result = expand_mentions(input, &dir);
        assert!(result.contains("file1"), "got: {result}");
        assert!(result.contains("file2"), "got: {result}");
        let _ = std::fs::remove_file(&f1);
        let _ = std::fs::remove_file(&f2);
    }

    #[test]
    fn file_at_helper_complete_no_at_returns_empty() {
        let helper = FileAtHelper { cwd: std::env::temp_dir() };
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, pairs) = helper.complete("hello world", 5, &ctx).unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn file_at_helper_complete_at_with_space_returns_empty() {
        let helper = FileAtHelper { cwd: std::env::temp_dir() };
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, pairs) = helper.complete("@ hello", 7, &ctx).unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn expand_mentions_at_end_of_string_is_lone_at() {
        let result = expand_mentions("end @", Path::new("/tmp"));
        assert_eq!(result, "end @");
    }

    #[test]
    fn expand_mentions_code_block_includes_filename_in_header() {
        let dir = std::env::temp_dir();
        let path = dir.join("trogon_header_test.txt");
        std::fs::write(&path, "content").unwrap();
        let result = expand_mentions("@trogon_header_test.txt", &dir);
        assert!(
            result.contains("`trogon_header_test.txt`"),
            "filename missing from header: {result}"
        );
        assert!(result.contains("```"), "missing fenced block: {result}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_at_helper_complete_filters_by_prefix() {
        let dir = std::env::temp_dir().join("trogon_compl_prefix");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("alpha.txt"), "").unwrap();
        std::fs::write(dir.join("alpha2.txt"), "").unwrap();
        std::fs::write(dir.join("beta.txt"), "").unwrap();

        let helper = FileAtHelper { cwd: dir.clone() };
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (start, pairs) = helper.complete("@alpha", 6, &ctx).unwrap();

        assert_eq!(start, 1, "start must be one past '@'");
        let names: Vec<&str> = pairs.iter().map(|p| p.display.as_str()).collect();
        assert!(names.contains(&"alpha.txt"), "expected alpha.txt in {names:?}");
        assert!(names.contains(&"alpha2.txt"), "expected alpha2.txt in {names:?}");
        assert!(!names.contains(&"beta.txt"), "beta.txt should be filtered out");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_at_helper_complete_directory_gets_slash_suffix() {
        let dir = std::env::temp_dir().join("trogon_compl_dir");
        let sub = dir.join("mysubdir");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&sub).unwrap();

        let helper = FileAtHelper { cwd: dir.clone() };
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, pairs) = helper.complete("@my", 3, &ctx).unwrap();

        let names: Vec<&str> = pairs.iter().map(|p| p.display.as_str()).collect();
        assert!(names.contains(&"mysubdir/"), "expected 'mysubdir/', got {names:?}");

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

        let helper = FileAtHelper { cwd: dir.clone() };
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, pairs) = helper.complete("@", 1, &ctx).unwrap();

        let names: Vec<&str> = pairs.iter().map(|p| p.display.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "completions should be alphabetically sorted");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── multiline join ────────────────────────────────────────────────────────

    #[test]
    fn join_continuation_single_line_unchanged() {
        assert_eq!(join_continuation("hello world"), "hello world");
    }

    #[test]
    fn join_continuation_backslash_newline_becomes_space() {
        let input = "line one\\\nline two\\\nline three";
        let result = join_continuation(input);
        assert_eq!(result, "line one line two line three");
    }

    #[test]
    fn join_continuation_no_backslash_newlines_unchanged() {
        let input = "line one\nline two";
        assert_eq!(join_continuation(input), "line one\nline two");
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

    // ── handle_slash_command ──────────────────────────────────────────────────

    #[test]
    fn slash_help_lists_all_commands() {
        let out = handle_slash_command("/help", "", 0, 0);
        assert!(out.contains("/help"));
        assert!(out.contains("/cost"));
        assert!(out.contains("/clear"));
        assert!(out.contains("/compact"));
        assert!(out.contains("/config"));
        assert!(out.contains("/model"));
        assert!(out.contains("/init"));
    }

    #[test]
    fn slash_cost_no_data_yet() {
        let out = handle_slash_command("/cost", "", 0, 0);
        assert!(out.contains("no usage data"), "got: {out}");
    }

    #[test]
    fn slash_cost_shows_percentage() {
        let out = handle_slash_command("/cost", "", 50_000, 200_000);
        assert!(out.contains("25%"), "got: {out}");
        assert!(out.contains("50,000"), "got: {out}");
        assert!(out.contains("200,000"), "got: {out}");
    }

    #[test]
    fn slash_compact_explains_auto() {
        let out = handle_slash_command("/compact", "", 0, 0);
        assert!(out.contains("automatic") || out.contains("85%"), "got: {out}");
    }

    #[test]
    fn slash_unknown_suggests_help() {
        let out = handle_slash_command("/nope", "", 0, 0);
        assert!(out.contains("unknown command"), "got: {out}");
        assert!(out.contains("/help"), "got: {out}");
    }

    #[test]
    fn slash_model_without_arg_shows_usage() {
        let out = handle_slash_command("/model", "", 0, 0);
        assert!(out.contains("usage") || out.contains("not yet"), "got: {out}");
    }

    // ── handle_config_cmd ─────────────────────────────────────────────────────

    #[test]
    fn config_set_and_get_roundtrip() {
        use std::sync::Mutex;
        // Isolate config file per test by overriding HOME to a temp dir.
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap();

        let dir = std::env::temp_dir().join("trogon_cfg_test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &dir);

        let set_out = handle_config_cmd("set mykey myvalue");
        assert!(set_out.contains("mykey"), "got: {set_out}");
        assert!(set_out.contains("myvalue"), "got: {set_out}");

        let get_out = handle_config_cmd("get mykey");
        assert!(get_out.contains("myvalue"), "got: {get_out}");

        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn config_get_missing_key_says_not_set() {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap();

        let dir = std::env::temp_dir().join("trogon_cfg_missing");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &dir);

        let out = handle_config_cmd("get nonexistent");
        assert!(out.contains("not set"), "got: {out}");

        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn config_no_args_shows_path() {
        let out = handle_config_cmd("");
        assert!(out.contains("config"), "got: {out}");
    }

    #[test]
    fn config_set_missing_key_shows_usage() {
        let out = handle_config_cmd("set");
        assert!(out.contains("usage"), "got: {out}");
    }

    #[test]
    fn config_get_missing_key_arg_shows_usage() {
        let out = handle_config_cmd("get");
        assert!(out.contains("usage"), "got: {out}");
    }
}
