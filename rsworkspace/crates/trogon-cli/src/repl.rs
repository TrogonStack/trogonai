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
    let session = TrogonSession::new(nats, prefix, cwd.clone()).await?;

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

                if let Some(output) = handle_slash_command(&line) {
                    println!("{output}");
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

fn handle_slash_command(line: &str) -> Option<String> {
    if !line.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    match parts[0] {
        "/help" => Some(
            "Commands: /help /clear /cost /model <id>\n\
             Multiline: end a line with \\ to continue on the next line\n\
             Ctrl+C  cancel active response    Ctrl+D  quit"
                .to_string(),
        ),
        "/clear" => Some("[/clear not yet implemented — PR 9]".to_string()),
        "/cost" => Some("[/cost not yet implemented — PR 9]".to_string()),
        _ => Some(format!("unknown command: {}", parts[0])),
    }
}

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
}
